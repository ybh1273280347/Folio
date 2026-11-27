//! Native in-process CUDA Driver API (`nvcuda.dll`) inference engine for Qwen3-0.6B RAG models.
//!
//! Directly loads `Qwen3-Embedding-0.6B-Q8_0.gguf` and `qwen3-reranker-0.6b-q8_0.gguf` onto any
//! NVIDIA SM >= 8.0 GPU using hand-tuned PTX 8.0 Tensor Core GEMM + Tensor Core FlashAttention-2
//! with shared prefix KV caching for reranking and a single shared VRAM activation workspace.
//! Falls back cleanly to `llama-server` when no compatible NVIDIA GPU or GGUF model is present.

use super::{
    chunker::shared_tokenizer,
    cuda_ptx::{build_module_ptx, f16_bits_to_f32, f32_to_f16_bits},
};
use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_int, c_uint, c_void},
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    os::windows::fs::FileExt,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const HIDDEN_DIM: usize = 1024;
const NUM_LAYERS: usize = 28;
const FFN_DIM: usize = 3072;
const QKV_DIM: usize = 4096;
const MAX_BATCH_TOKENS: usize = 4608;
const MAX_ROPE_POS: usize = 4096;
const MAX_PREFIX_TOKENS: usize = 512;
const Q8_0_BLOCK_BYTES: usize = 34;
const EMBED_ROW_Q8_BYTES: u64 = (HIDDEN_DIM as u64 / 32) * Q8_0_BLOCK_BYTES as u64; // 1088 B
const MAX_EMB_CACHE_ENTRIES: usize = 16_384;

const TOKEN_ENDOFTEXT: u32 = 151_643;
const TOKEN_IM_START: u32 = 151_644;
const TOKEN_IM_END: u32 = 151_645;
const TOKEN_THINK_OPEN: u32 = 151_667;
const TOKEN_THINK_CLOSE: u32 = 151_668;

const QUERY_INSTRUCT: &str =
    "Instruct: Given a search query, retrieve relevant passages from personal notes and papers\nQuery: ";

const CU_DEVICE_ATTR_CC_MAJOR: c_int = 75;
const CU_JIT_ERROR_LOG_BUFFER: c_int = 5;
const CU_JIT_ERROR_LOG_BUFFER_SIZE_BYTES: c_int = 6;

type CuResult = c_int;
type CuDevice = c_int;
type CuContext = *mut c_void;
type CuModule = *mut c_void;
type CuFunction = *mut c_void;
type CuDevicePtr = u64;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
    fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const c_char) -> *mut c_void;
}

struct CudaApi {
    _lib: *mut c_void,
    cu_init: unsafe extern "system" fn(c_uint) -> CuResult,
    cu_device_get: unsafe extern "system" fn(*mut CuDevice, c_int) -> CuResult,
    cu_device_get_attribute: unsafe extern "system" fn(*mut c_int, c_int, CuDevice) -> CuResult,
    cu_ctx_create: unsafe extern "system" fn(*mut CuContext, c_uint, CuDevice) -> CuResult,
    cu_ctx_set_current: unsafe extern "system" fn(CuContext) -> CuResult,
    cu_ctx_synchronize: unsafe extern "system" fn() -> CuResult,
    cu_module_load_data_ex: unsafe extern "system" fn(
        *mut CuModule,
        *const c_void,
        c_uint,
        *const c_int,
        *const *mut c_void,
    ) -> CuResult,
    cu_module_get_function:
        unsafe extern "system" fn(*mut CuFunction, CuModule, *const c_char) -> CuResult,
    cu_mem_alloc: unsafe extern "system" fn(*mut CuDevicePtr, usize) -> CuResult,
    cu_mem_free: unsafe extern "system" fn(CuDevicePtr) -> CuResult,
    cu_memcpy_htod: unsafe extern "system" fn(CuDevicePtr, *const c_void, usize) -> CuResult,
    cu_memcpy_dtoh: unsafe extern "system" fn(*mut c_void, CuDevicePtr, usize) -> CuResult,
    cu_launch_kernel: unsafe extern "system" fn(
        CuFunction,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        *mut c_void,
        *mut *mut c_void,
        *mut *mut c_void,
    ) -> CuResult,
}

unsafe impl Send for CudaApi {}
unsafe impl Sync for CudaApi {}

impl CudaApi {
    fn load() -> Result<Self, String> {
        let wide: Vec<u16> = "nvcuda.dll\0".encode_utf16().collect();
        let lib = unsafe { LoadLibraryW(wide.as_ptr()) };
        if lib.is_null() {
            return Err("nvcuda.dll not found".to_owned());
        }
        macro_rules! sym {
            ($name:literal) => {{
                let cname = concat!($name, "\0");
                let ptr = unsafe { GetProcAddress(lib, cname.as_ptr() as *const c_char) };
                if ptr.is_null() {
                    return Err(format!("missing CUDA symbol {}", $name));
                }
                unsafe { std::mem::transmute(ptr) }
            }};
        }
        Ok(Self {
            _lib: lib,
            cu_init: sym!("cuInit"),
            cu_device_get: sym!("cuDeviceGet"),
            cu_device_get_attribute: sym!("cuDeviceGetAttribute"),
            cu_ctx_create: sym!("cuCtxCreate_v2"),
            cu_ctx_set_current: sym!("cuCtxSetCurrent"),
            cu_ctx_synchronize: sym!("cuCtxSynchronize"),
            cu_module_load_data_ex: sym!("cuModuleLoadDataEx"),
            cu_module_get_function: sym!("cuModuleGetFunction"),
            cu_mem_alloc: sym!("cuMemAlloc_v2"),
            cu_mem_free: sym!("cuMemFree_v2"),
            cu_memcpy_htod: sym!("cuMemcpyHtoD_v2"),
            cu_memcpy_dtoh: sym!("cuMemcpyDtoH_v2"),
            cu_launch_kernel: sym!("cuLaunchKernel"),
        })
    }

    #[inline]
    fn check(res: CuResult, op: &str) -> Result<(), String> {
        if res == 0 {
            Ok(())
        } else {
            Err(format!("CUDA error {res} at {op}"))
        }
    }

    fn alloc(&self, bytes: usize) -> Result<CuDevicePtr, String> {
        let mut ptr: CuDevicePtr = 0;
        Self::check(unsafe { (self.cu_mem_alloc)(&mut ptr, bytes.max(16)) }, "cuMemAlloc")?;
        Ok(ptr)
    }

    fn free(&self, ptr: CuDevicePtr) {
        if ptr != 0 {
            unsafe {
                let _ = (self.cu_mem_free)(ptr);
            }
        }
    }

    fn h2d<T>(&self, dst: CuDevicePtr, src: &[T]) -> Result<(), String> {
        let bytes = std::mem::size_of_val(src);
        if bytes == 0 {
            return Ok(());
        }
        Self::check(
            unsafe { (self.cu_memcpy_htod)(dst, src.as_ptr() as *const c_void, bytes) },
            "cuMemcpyHtoD",
        )
    }

    fn d2h<T>(&self, dst: &mut [T], src: CuDevicePtr) -> Result<(), String> {
        let bytes = std::mem::size_of_val(dst);
        if bytes == 0 {
            return Ok(());
        }
        Self::check(
            unsafe { (self.cu_memcpy_dtoh)(dst.as_mut_ptr() as *mut c_void, src, bytes) },
            "cuMemcpyDtoH",
        )
    }

    fn sync(&self) -> Result<(), String> {
        Self::check(unsafe { (self.cu_ctx_synchronize)() }, "cuCtxSynchronize")
    }

    #[allow(clippy::too_many_arguments)]
    fn launch(
        &self,
        func: CuFunction,
        grid: (u32, u32, u32),
        block: (u32, u32, u32),
        shared_bytes: u32,
        params: &mut [*mut c_void],
    ) -> Result<(), String> {
        Self::check(
            unsafe {
                (self.cu_launch_kernel)(
                    func,
                    grid.0,
                    grid.1,
                    grid.2,
                    block.0,
                    block.1,
                    block.2,
                    shared_bytes,
                    std::ptr::null_mut(),
                    params.as_mut_ptr(),
                    std::ptr::null_mut(),
                )
            },
            "cuLaunchKernel",
        )
    }
}

struct KernelSet {
    dequant_q8: CuFunction,
    gemm_128: CuFunction,
    gemm_128_res: CuFunction,
    gemm_64: CuFunction,
    gemm_64_res: CuFunction,
    rmsnorm_1024: CuFunction,
    qk_norm_rope: CuFunction,
    copy_prefix_kv: CuFunction,
    flash_attn_tc: CuFunction,
    swiglu_3072: CuFunction,
}

unsafe impl Send for KernelSet {}

struct SlabAllocator {
    cursor: usize,
}

impl SlabAllocator {
    fn new() -> Self {
        Self { cursor: 0 }
    }
    fn reserve(&mut self, bytes: usize) -> usize {
        let aligned = (self.cursor + 255) & !255;
        self.cursor = aligned + bytes;
        aligned
    }
    fn total_bytes(&self) -> usize {
        self.cursor
    }
}

struct SharedWorkspace {
    d_slab: CuDevicePtr,
    d_cos: CuDevicePtr,
    d_sin: CuDevicePtr,
    d_w_f16: CuDevicePtr,
    d_hidden: CuDevicePtr,
    d_norm: CuDevicePtr,
    d_big: CuDevicePtr,
    d_mid: CuDevicePtr,
    d_pos: CuDevicePtr,
    d_tiles: CuDevicePtr,
    d_pref_k: Vec<CuDevicePtr>,
    d_pref_v: Vec<CuDevicePtr>,
    host_in_f16: Vec<u16>,
    host_pos: Vec<u32>,
    host_tiles: Vec<u32>,
}

impl SharedWorkspace {
    fn new(api: &CudaApi) -> Result<Self, String> {
        let mut cos_table = vec![0.0_f32; MAX_ROPE_POS * 64];
        let mut sin_table = vec![0.0_f32; MAX_ROPE_POS * 64];
        let inv_freq: Vec<f64> = (0..64)
            .map(|j| 1.0_f64 / 1_000_000.0_f64.powf(2.0 * (j as f64) / 128.0))
            .collect();
        for p in 0..MAX_ROPE_POS {
            let pf = p as f64;
            let base = p * 64;
            for j in 0..64 {
                let ang = pf * inv_freq[j];
                cos_table[base + j] = ang.cos() as f32;
                sin_table[base + j] = ang.sin() as f32;
            }
        }

        // Single WDDM cuMemAlloc_v2 for the entire shared workspace!
        let mut slab = SlabAllocator::new();
        let o_cos = slab.reserve(cos_table.len() * 4);
        let o_sin = slab.reserve(sin_table.len() * 4);
        let o_w_f16 = slab.reserve(6144 * HIDDEN_DIM * 2);
        let o_hidden = slab.reserve(MAX_BATCH_TOKENS * HIDDEN_DIM * 2);
        let o_norm = slab.reserve(MAX_BATCH_TOKENS * HIDDEN_DIM * 2);
        let o_big = slab.reserve(MAX_BATCH_TOKENS * 6144 * 2);
        let o_mid = slab.reserve(MAX_BATCH_TOKENS * FFN_DIM * 2);
        let o_pos = slab.reserve(MAX_BATCH_TOKENS * 4);
        let o_tiles = slab.reserve(MAX_BATCH_TOKENS * 12);
        let mut o_pref_k = Vec::with_capacity(NUM_LAYERS);
        let mut o_pref_v = Vec::with_capacity(NUM_LAYERS);
        for _ in 0..NUM_LAYERS {
            o_pref_k.push(slab.reserve(MAX_PREFIX_TOKENS * HIDDEN_DIM * 2));
            o_pref_v.push(slab.reserve(MAX_PREFIX_TOKENS * HIDDEN_DIM * 2));
        }

        let d_slab = api.alloc(slab.total_bytes())?;
        let d_cos = d_slab + o_cos as u64;
        let d_sin = d_slab + o_sin as u64;
        api.h2d(d_cos, &cos_table)?;
        api.h2d(d_sin, &sin_table)?;

        Ok(Self {
            d_slab,
            d_cos,
            d_sin,
            d_w_f16: d_slab + o_w_f16 as u64,
            d_hidden: d_slab + o_hidden as u64,
            d_norm: d_slab + o_norm as u64,
            d_big: d_slab + o_big as u64,
            d_mid: d_slab + o_mid as u64,
            d_pos: d_slab + o_pos as u64,
            d_tiles: d_slab + o_tiles as u64,
            d_pref_k: o_pref_k.into_iter().map(|o| d_slab + o as u64).collect(),
            d_pref_v: o_pref_v.into_iter().map(|o| d_slab + o as u64).collect(),
            host_in_f16: Vec::with_capacity(MAX_BATCH_TOKENS * HIDDEN_DIM),
            host_pos: Vec::with_capacity(MAX_BATCH_TOKENS),
            host_tiles: Vec::with_capacity(MAX_BATCH_TOKENS * 3),
        })
    }

    fn free_all(&mut self, api: &CudaApi) {
        api.free(self.d_slab);
        self.d_slab = 0;
    }
}

struct Q8Weight {
    d_qs: CuDevicePtr,
    d_d: CuDevicePtr,
    n_elems: u32,
}

struct LayerWeights {
    attn_norm: CuDevicePtr,
    q_norm: CuDevicePtr,
    k_norm: CuDevicePtr,
    ffn_norm: CuDevicePtr,
    qkv: Q8Weight,
    attn_o: Q8Weight,
    gate_up: Q8Weight,
    ffn_down: Q8Weight,
}

struct LoadedModel {
    is_reranker: bool,
    file: File,
    token_embd_file_offset: u64,
    vocab_size: usize,
    emb_cache: HashMap<u32, Box<[u16; HIDDEN_DIM]>>,
    output_norm_w: Vec<f32>,
    cls_w: Option<[Vec<f32>; 2]>,
    d_weights_slab: CuDevicePtr,
    layers: Vec<LayerWeights>,
    last_used: Instant,
    idle_release_secs: u64,
}

impl LoadedModel {
    fn free_gpu(&mut self, api: &CudaApi) {
        api.free(self.d_weights_slab);
        self.d_weights_slab = 0;
        self.layers.clear();
    }

    fn gather_token_embeddings(&mut self, token_ids: &[u32], out: &mut Vec<u16>) -> Result<(), String> {
        out.clear();
        out.reserve(token_ids.len() * HIDDEN_DIM);
        let mut raw_row = [0u8; EMBED_ROW_Q8_BYTES as usize];
        for &tid in token_ids {
            let safe_tid = if (tid as usize) < self.vocab_size { tid } else { 0 };
            if let Some(cached) = self.emb_cache.get(&safe_tid) {
                out.extend_from_slice(&cached[..]);
                continue;
            }
            let offset = self.token_embd_file_offset + (safe_tid as u64) * EMBED_ROW_Q8_BYTES;
            read_exact_at(&self.file, offset, &mut raw_row)?;
            let mut row = Box::new([0u16; HIDDEN_DIM]);
            for b in 0..(HIDDEN_DIM / 32) {
                let bo = b * Q8_0_BLOCK_BYTES;
                let d_bits = u16::from_le_bytes([raw_row[bo], raw_row[bo + 1]]);
                let d = f16_bits_to_f32(d_bits);
                let out_base = b * 32;
                for j in 0..32 {
                    let q = raw_row[bo + 2 + j] as i8 as f32;
                    row[out_base + j] = f32_to_f16_bits(q * d);
                }
            }
            out.extend_from_slice(&row[..]);
            if self.emb_cache.len() < MAX_EMB_CACHE_ENTRIES {
                self.emb_cache.insert(safe_tid, row);
            }
        }
        Ok(())
    }
}

fn read_exact_at(file: &File, mut offset: u64, mut buf: &mut [u8]) -> Result<(), String> {
    while !buf.is_empty() {
        let n = file
            .seek_read(buf, offset)
            .map_err(|e| format!("读取 GGUF 失败: {e}"))?;
        if n == 0 {
            return Err("GGUF 文件提前结束".to_owned());
        }
        offset += n as u64;
        buf = &mut buf[n..];
    }
    Ok(())
}

struct GgufTensorMeta {
    dims: Vec<u64>,
    ggml_type: u32,
    rel_offset: u64,
}

struct GgufHeader {
    arch: String,
    block_count: u32,
    embedding_length: u32,
    ffn_length: u32,
    head_count: u32,
    head_count_kv: u32,
    key_length: u32,
    pooling_type: u32,
    data_offset: u64,
    tensors: HashMap<String, GgufTensorMeta>,
}

fn read_u32<R: Read>(r: &mut R) -> Result<u32, String> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).map_err(|e| e.to_string())?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64<R: Read>(r: &mut R) -> Result<u64, String> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).map_err(|e| e.to_string())?;
    Ok(u64::from_le_bytes(b))
}

fn read_gguf_str<R: Read>(r: &mut R) -> Result<String, String> {
    let len = read_u64(r)? as usize;
    if len > 16 * 1024 * 1024 {
        return Err("GGUF string too large".to_owned());
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn skip_gguf_val<R: Read + Seek>(r: &mut R, vtype: u32) -> Result<(), String> {
    let fixed: Option<i64> = match vtype {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4 | 5 | 6 => Some(4),
        10 | 11 | 12 => Some(8),
        _ => None,
    };
    if let Some(sz) = fixed {
        r.seek(SeekFrom::Current(sz)).map_err(|e| e.to_string())?;
        return Ok(());
    }
    match vtype {
        8 => {
            let len = read_u64(r)? as i64;
            r.seek(SeekFrom::Current(len)).map_err(|e| e.to_string())?;
        }
        9 => {
            let elem_t = read_u32(r)?;
            let count = read_u64(r)?;
            let elem_sz: Option<u64> = match elem_t {
                0 | 1 | 7 => Some(1),
                2 | 3 => Some(2),
                4 | 5 | 6 => Some(4),
                10 | 11 | 12 => Some(8),
                _ => None,
            };
            if let Some(sz) = elem_sz {
                r.seek(SeekFrom::Current((sz * count) as i64))
                    .map_err(|e| e.to_string())?;
            } else {
                for _ in 0..count {
                    skip_gguf_val(r, elem_t)?;
                }
            }
        }
        other => return Err(format!("unknown GGUF value type {other}")),
    }
    Ok(())
}

fn parse_gguf_header(path: &Path) -> Result<GgufHeader, String> {
    let file = File::open(path).map_err(|e| format!("打开模型失败: {e}"))?;
    let mut r = BufReader::with_capacity(64 * 1024, file);
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).map_err(|e| e.to_string())?;
    if &magic != b"GGUF" {
        return Err("不是有效的 GGUF 文件".to_owned());
    }
    let _ver = read_u32(&mut r)?;
    let n_tensors = read_u64(&mut r)?;
    let n_kv = read_u64(&mut r)?;

    let mut arch = String::new();
    let mut block_count = 0u32;
    let mut embedding_length = 0u32;
    let mut ffn_length = 0u32;
    let mut head_count = 0u32;
    let mut head_count_kv = 0u32;
    let mut key_length = 0u32;
    let mut pooling_type = 0u32;
    let mut alignment = 32u64;

    for _ in 0..n_kv {
        let key = read_gguf_str(&mut r)?;
        let vtype = read_u32(&mut r)?;
        match (key.as_str(), vtype) {
            ("general.architecture", 8) => arch = read_gguf_str(&mut r)?,
            ("general.alignment", 4) => alignment = read_u32(&mut r)? as u64,
            ("qwen3.block_count", 4) => block_count = read_u32(&mut r)?,
            ("qwen3.embedding_length", 4) => embedding_length = read_u32(&mut r)?,
            ("qwen3.feed_forward_length", 4) => ffn_length = read_u32(&mut r)?,
            ("qwen3.attention.head_count", 4) => head_count = read_u32(&mut r)?,
            ("qwen3.attention.head_count_kv", 4) => head_count_kv = read_u32(&mut r)?,
            ("qwen3.attention.key_length", 4) => key_length = read_u32(&mut r)?,
            ("qwen3.pooling_type", 4) => pooling_type = read_u32(&mut r)?,
            _ => skip_gguf_val(&mut r, vtype)?,
        }
    }

    let mut tensors = HashMap::with_capacity(n_tensors as usize);
    for _ in 0..n_tensors {
        let name = read_gguf_str(&mut r)?;
        let ndims = read_u32(&mut r)? as usize;
        let mut dims = Vec::with_capacity(ndims);
        for _ in 0..ndims {
            dims.push(read_u64(&mut r)?);
        }
        let ggml_type = read_u32(&mut r)?;
        let rel_offset = read_u64(&mut r)?;
        tensors.insert(
            name,
            GgufTensorMeta {
                dims,
                ggml_type,
                rel_offset,
            },
        );
    }

    let pos = r.stream_position().map_err(|e| e.to_string())?;
    let data_offset = pos.div_ceil(alignment) * alignment;

    Ok(GgufHeader {
        arch,
        block_count,
        embedding_length,
        ffn_length,
        head_count,
        head_count_kv,
        key_length,
        pooling_type,
        data_offset,
        tensors,
    })
}

fn validate_qwen3_q8_header(hdr: &GgufHeader, is_reranker: bool) -> Result<(), String> {
    if hdr.arch != "qwen3"
        || hdr.block_count as usize != NUM_LAYERS
        || hdr.embedding_length as usize != HIDDEN_DIM
        || hdr.ffn_length as usize != FFN_DIM
        || hdr.head_count != 16
        || hdr.head_count_kv != 8
        || hdr.key_length != 128
    {
        return Err("仅支持 Qwen3-0.6B (28L/1024d/3072ff/16x8H) 架构".to_owned());
    }
    let expected_pool = if is_reranker { 4 } else { 3 };
    if hdr.pooling_type != expected_pool {
        return Err(format!("pooling_type={} 不匹配期望 {expected_pool}", hdr.pooling_type));
    }
    // Check required tensors are Q8_0 (type 8) / F32 (type 0)
    let check_t = |name: &str, expected_type: u32| -> Result<(), String> {
        let t = hdr
            .tensors
            .get(name)
            .ok_or_else(|| format!("缺少张量 {name}"))?;
        if t.ggml_type != expected_type {
            return Err(format!("张量 {name} 类型 {} 非预期 {expected_type}", t.ggml_type));
        }
        Ok(())
    };
    check_t("token_embd.weight", 8)?;
    check_t("output_norm.weight", 0)?;
    if is_reranker {
        check_t("cls.output.weight", 8)?;
    }
    for l in 0..NUM_LAYERS {
        check_t(&format!("blk.{l}.attn_norm.weight"), 0)?;
        check_t(&format!("blk.{l}.attn_q_norm.weight"), 0)?;
        check_t(&format!("blk.{l}.attn_k_norm.weight"), 0)?;
        check_t(&format!("blk.{l}.ffn_norm.weight"), 0)?;
        check_t(&format!("blk.{l}.attn_q.weight"), 8)?;
        check_t(&format!("blk.{l}.attn_k.weight"), 8)?;
        check_t(&format!("blk.{l}.attn_v.weight"), 8)?;
        check_t(&format!("blk.{l}.attn_output.weight"), 8)?;
        check_t(&format!("blk.{l}.ffn_gate.weight"), 8)?;
        check_t(&format!("blk.{l}.ffn_up.weight"), 8)?;
        check_t(&format!("blk.{l}.ffn_down.weight"), 8)?;
    }
    Ok(())
}

struct CudaRuntime {
    api: CudaApi,
    ctx: CuContext,
    _module: CuModule,
    kernels: KernelSet,
    ws: Option<SharedWorkspace>,
    models: HashMap<PathBuf, LoadedModel>,
}

unsafe impl Send for CudaRuntime {}

impl CudaRuntime {
    fn init() -> Result<Self, String> {
        let api = CudaApi::load()?;
        CudaApi::check(unsafe { (api.cu_init)(0) }, "cuInit")?;
        let mut dev: CuDevice = 0;
        CudaApi::check(unsafe { (api.cu_device_get)(&mut dev, 0) }, "cuDeviceGet")?;
        let mut major: c_int = 0;
        CudaApi::check(
            unsafe { (api.cu_device_get_attribute)(&mut major, CU_DEVICE_ATTR_CC_MAJOR, dev) },
            "CC_MAJOR",
        )?;
        if major < 8 {
            return Err(format!("GPU compute capability sm_{major}x < sm_80"));
        }

        let mut ctx: CuContext = std::ptr::null_mut();
        CudaApi::check(unsafe { (api.cu_ctx_create)(&mut ctx, 0, dev) }, "cuCtxCreate")?;

        let ptx = build_module_ptx();
        let ptx_c = CString::new(ptx).map_err(|e| e.to_string())?;
        let mut err_buf = vec![0u8; 16_384];
        let opts = [CU_JIT_ERROR_LOG_BUFFER, CU_JIT_ERROR_LOG_BUFFER_SIZE_BYTES];
        let vals = [
            err_buf.as_mut_ptr() as *mut c_void,
            err_buf.len() as *mut c_void,
        ];
        let mut module: CuModule = std::ptr::null_mut();
        let res = unsafe {
            (api.cu_module_load_data_ex)(
                &mut module,
                ptx_c.as_ptr() as *const c_void,
                opts.len() as c_uint,
                opts.as_ptr(),
                vals.as_ptr(),
            )
        };
        if res != 0 {
            let log = CStr::from_bytes_until_nul(&err_buf)
                .map(|c| c.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Err(format!("PTX JIT failed ({res}): {log}"));
        }

        let get_fn = |name: &str| -> Result<CuFunction, String> {
            let cname = CString::new(name).unwrap();
            let mut f: CuFunction = std::ptr::null_mut();
            CudaApi::check(
                unsafe { (api.cu_module_get_function)(&mut f, module, cname.as_ptr()) },
                name,
            )?;
            Ok(f)
        };

        let kernels = KernelSet {
            dequant_q8: get_fn("dequant_soa_q8_f16")?,
            gemm_128: get_fn("gemm_f16_128x128_2s")?,
            gemm_128_res: get_fn("gemm_f16_128x128_2s_res")?,
            gemm_64: get_fn("gemm_f16_64x128_3s")?,
            gemm_64_res: get_fn("gemm_f16_64x128_3s_res")?,
            rmsnorm_1024: get_fn("rmsnorm_1024_f16")?,
            qk_norm_rope: get_fn("qk_norm_rope_f16")?,
            copy_prefix_kv: get_fn("copy_prefix_kv_f16")?,
            flash_attn_tc: get_fn("flash_attn_tc_varlen_f16")?,
            swiglu_3072: get_fn("swiglu_3072_fast_f16")?,
        };

        Ok(Self {
            api,
            ctx,
            _module: module,
            kernels,
            ws: None,
            models: HashMap::new(),
        })
    }

    fn ensure_ws(&mut self) -> Result<&mut SharedWorkspace, String> {
        if self.ws.is_none() {
            self.ws = Some(SharedWorkspace::new(&self.api)?);
        }
        Ok(self.ws.as_mut().expect("workspace initialized"))
    }

    fn bind_ctx(&self) -> Result<(), String> {
        CudaApi::check(unsafe { (self.api.cu_ctx_set_current)(self.ctx) }, "cuCtxSetCurrent")
    }

    fn ensure_model_loaded(
        &mut self,
        path: &Path,
        is_reranker: bool,
        idle_release_secs: u64,
    ) -> Result<PathBuf, String> {
        self.bind_ctx()?;
        let key = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(m) = self.models.get_mut(&key) {
            if m.is_reranker == is_reranker {
                m.last_used = Instant::now();
                m.idle_release_secs = idle_release_secs;
                return Ok(key);
            }
        }

        let hdr = parse_gguf_header(&key)?;
        validate_qwen3_q8_header(&hdr, is_reranker)?;

        let file = File::open(&key).map_err(|e| format!("打开模型失败: {e}"))?;
        let tensor_bytes_info = |name: &str| -> Result<(u64, usize), String> {
            let t = hdr.tensors.get(name).ok_or_else(|| format!("缺少张量 {name}"))?;
            let ne: u64 = t.dims.iter().product();
            let nb = if t.ggml_type == 0 {
                (ne * 4) as usize
            } else {
                ((ne / 32) * (Q8_0_BLOCK_BYTES as u64)) as usize
            };
            Ok((hdr.data_offset + t.rel_offset, nb))
        };

        let emb_meta = hdr
            .tensors
            .get("token_embd.weight")
            .ok_or_else(|| "缺少 token_embd.weight".to_owned())?;
        let token_embd_file_offset = hdr.data_offset + emb_meta.rel_offset;
        let vocab_size = *emb_meta.dims.get(1).unwrap_or(&151_669) as usize;

        // Read output_norm.weight (4 KB)
        let (on_off, on_len) = tensor_bytes_info("output_norm.weight")?;
        let mut on_raw = vec![0u8; on_len];
        read_exact_at(&file, on_off, &mut on_raw)?;
        let output_norm_w: Vec<f32> = on_raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        // Read & dequant cls.output.weight [2, 1024] Q8_0 on CPU once for reranker
        let cls_w = if is_reranker {
            let (cls_off, cls_len) = tensor_bytes_info("cls.output.weight")?;
            let mut raw = vec![0u8; cls_len];
            read_exact_at(&file, cls_off, &mut raw)?;
            let mut rows = [vec![0.0_f32; HIDDEN_DIM], vec![0.0_f32; HIDDEN_DIM]];
            for (r, row) in rows.iter_mut().enumerate() {
                for b in 0..(HIDDEN_DIM / 32) {
                    let bo = (r * (HIDDEN_DIM / 32) + b) * Q8_0_BLOCK_BYTES;
                    let d = f16_bits_to_f32(u16::from_le_bytes([raw[bo], raw[bo + 1]]));
                    for j in 0..32 {
                        row[b * 32 + j] = (raw[bo + 2 + j] as i8 as f32) * d;
                    }
                }
            }
            Some(rows)
        } else {
            None
        };

        // Pre-compute single-slab GPU layout for all 28 layers -> ONE cuMemAlloc_v2 instead of 336!
        let mut slab = SlabAllocator::new();
        struct LayerOffsets {
            attn_norm: usize,
            q_norm: usize,
            k_norm: usize,
            ffn_norm: usize,
            qkv_qs: usize,
            qkv_d: usize,
            qkv_ne: u32,
            ao_qs: usize,
            ao_d: usize,
            ao_ne: u32,
            gu_qs: usize,
            gu_d: usize,
            gu_ne: u32,
            fd_qs: usize,
            fd_d: usize,
            fd_ne: u32,
        }
        let mut loffs = Vec::with_capacity(NUM_LAYERS);
        let mut min_blk_off = u64::MAX;
        let mut max_blk_end = 0u64;
        for l in 0..NUM_LAYERS {
            let names = [
                format!("blk.{l}.attn_norm.weight"),
                format!("blk.{l}.attn_q_norm.weight"),
                format!("blk.{l}.attn_k_norm.weight"),
                format!("blk.{l}.ffn_norm.weight"),
                format!("blk.{l}.attn_q.weight"),
                format!("blk.{l}.attn_k.weight"),
                format!("blk.{l}.attn_v.weight"),
                format!("blk.{l}.attn_output.weight"),
                format!("blk.{l}.ffn_gate.weight"),
                format!("blk.{l}.ffn_up.weight"),
                format!("blk.{l}.ffn_down.weight"),
            ];
            let mut infos = [(0u64, 0usize); 11];
            for (i, n) in names.iter().enumerate() {
                let (off, len) = tensor_bytes_info(n)?;
                infos[i] = (off, len);
                min_blk_off = min_blk_off.min(off);
                max_blk_end = max_blk_end.max(off + len as u64);
            }
            let qkv_nb = (infos[4].1 + infos[5].1 + infos[6].1) / Q8_0_BLOCK_BYTES;
            let ao_nb = infos[7].1 / Q8_0_BLOCK_BYTES;
            let gu_nb = (infos[8].1 + infos[9].1) / Q8_0_BLOCK_BYTES;
            let fd_nb = infos[10].1 / Q8_0_BLOCK_BYTES;
            loffs.push(LayerOffsets {
                attn_norm: slab.reserve(infos[0].1),
                q_norm: slab.reserve(infos[1].1),
                k_norm: slab.reserve(infos[2].1),
                ffn_norm: slab.reserve(infos[3].1),
                qkv_qs: slab.reserve(qkv_nb * 32),
                qkv_d: slab.reserve(qkv_nb * 2),
                qkv_ne: (qkv_nb * 32) as u32,
                ao_qs: slab.reserve(ao_nb * 32),
                ao_d: slab.reserve(ao_nb * 2),
                ao_ne: (ao_nb * 32) as u32,
                gu_qs: slab.reserve(gu_nb * 32),
                gu_d: slab.reserve(gu_nb * 2),
                gu_ne: (gu_nb * 32) as u32,
                fd_qs: slab.reserve(fd_nb * 32),
                fd_d: slab.reserve(fd_nb * 2),
                fd_ne: (fd_nb * 32) as u32,
            });
        }

        // One sequential read of the contiguous blk.* region (~467 MB, skipping 155 MB token_embd!)
        let region_len = (max_blk_end - min_blk_off) as usize;
        let mut region = vec![0u8; region_len];
        read_exact_at(&file, min_blk_off, &mut region)?;

        // Repack all 28 layers into ONE contiguous host slab (SoA Q8_0 + F32 norms), then ONE single H2D DMA!
        let mut host_slab = vec![0u8; slab.total_bytes()];
        let slice_of = |name: &str| -> Result<&[u8], String> {
            let (off, len) = tensor_bytes_info(name)?;
            let rel = (off - min_blk_off) as usize;
            Ok(&region[rel..rel + len])
        };
        let pack_q8_soa = |names: &[&str], dst_qs_off: usize, dst_d_off: usize, dst: &mut [u8]| -> Result<(), String> {
            let mut blk_idx = 0usize;
            for name in names {
                let raw = slice_of(name)?;
                for blk in raw.chunks_exact(Q8_0_BLOCK_BYTES) {
                    let d_pos = dst_d_off + blk_idx * 2;
                    dst[d_pos] = blk[0];
                    dst[d_pos + 1] = blk[1];
                    let qs_pos = dst_qs_off + blk_idx * 32;
                    dst[qs_pos..qs_pos + 32].copy_from_slice(&blk[2..34]);
                    blk_idx += 1;
                }
            }
            Ok(())
        };

        for (l, lo) in loffs.iter().enumerate() {
            for (name, off) in [
                (format!("blk.{l}.attn_norm.weight"), lo.attn_norm),
                (format!("blk.{l}.attn_q_norm.weight"), lo.q_norm),
                (format!("blk.{l}.attn_k_norm.weight"), lo.k_norm),
                (format!("blk.{l}.ffn_norm.weight"), lo.ffn_norm),
            ] {
                let s = slice_of(&name)?;
                host_slab[off..off + s.len()].copy_from_slice(s);
            }
            pack_q8_soa(
                &[
                    &format!("blk.{l}.attn_q.weight"),
                    &format!("blk.{l}.attn_k.weight"),
                    &format!("blk.{l}.attn_v.weight"),
                ],
                lo.qkv_qs,
                lo.qkv_d,
                &mut host_slab,
            )?;
            pack_q8_soa(
                &[&format!("blk.{l}.attn_output.weight")],
                lo.ao_qs,
                lo.ao_d,
                &mut host_slab,
            )?;
            pack_q8_soa(
                &[&format!("blk.{l}.ffn_gate.weight"), &format!("blk.{l}.ffn_up.weight")],
                lo.gu_qs,
                lo.gu_d,
                &mut host_slab,
            )?;
            pack_q8_soa(
                &[&format!("blk.{l}.ffn_down.weight")],
                lo.fd_qs,
                lo.fd_d,
                &mut host_slab,
            )?;
        }
        drop(region);

        let d_weights_slab = self.api.alloc(host_slab.len())?;
        if let Err(err) = self.api.h2d(d_weights_slab, &host_slab) {
            self.api.free(d_weights_slab);
            return Err(err);
        }

        let mut layers = Vec::with_capacity(NUM_LAYERS);
        for lo in &loffs {
            layers.push(LayerWeights {
                attn_norm: d_weights_slab + lo.attn_norm as u64,
                q_norm: d_weights_slab + lo.q_norm as u64,
                k_norm: d_weights_slab + lo.k_norm as u64,
                ffn_norm: d_weights_slab + lo.ffn_norm as u64,
                qkv: Q8Weight {
                    d_qs: d_weights_slab + lo.qkv_qs as u64,
                    d_d: d_weights_slab + lo.qkv_d as u64,
                    n_elems: lo.qkv_ne,
                },
                attn_o: Q8Weight {
                    d_qs: d_weights_slab + lo.ao_qs as u64,
                    d_d: d_weights_slab + lo.ao_d as u64,
                    n_elems: lo.ao_ne,
                },
                gate_up: Q8Weight {
                    d_qs: d_weights_slab + lo.gu_qs as u64,
                    d_d: d_weights_slab + lo.gu_d as u64,
                    n_elems: lo.gu_ne,
                },
                ffn_down: Q8Weight {
                    d_qs: d_weights_slab + lo.fd_qs as u64,
                    d_d: d_weights_slab + lo.fd_d as u64,
                    n_elems: lo.fd_ne,
                },
            });
        }

        self.models.insert(
            key.clone(),
            LoadedModel {
                is_reranker,
                file,
                token_embd_file_offset,
                vocab_size,
                emb_cache: HashMap::with_capacity(2048),
                output_norm_w,
                cls_w,
                d_weights_slab,
                layers,
                last_used: Instant::now(),
                idle_release_secs,
            },
        );
        Ok(key)
    }

    fn forward_packed(
        &mut self,
        model_key: &Path,
        seqs: &[&[u32]],
        save_prefix: bool,
        use_prefix_len: u32,
    ) -> Result<Vec<[u16; HIDDEN_DIM]>, String> {
        self.bind_ctx()?;
        let _ = self.ensure_ws()?;
        let ws = self.ws.as_mut().expect("workspace initialized");
        let model = self
            .models
            .get_mut(model_key)
            .ok_or_else(|| "CUDA 模型未加载".to_owned())?;
        model.last_used = Instant::now();

        let total_t: usize = seqs.iter().map(|s| s.len()).sum();
        if total_t == 0 || total_t > MAX_BATCH_TOKENS {
            return Err(format!("无效的批大小 total_t={total_t}"));
        }

        let mut flat_ids = Vec::with_capacity(total_t);
        ws.host_pos.clear();
        ws.host_tiles.clear();
        let mut last_indices = Vec::with_capacity(seqs.len());
        let mut cur = 0u32;
        for s in seqs {
            let sl = s.len() as u32;
            flat_ids.extend_from_slice(s);
            for p in 0..sl {
                ws.host_pos
                    .push((use_prefix_len + p).min((MAX_ROPE_POS - 1) as u32));
            }
            let mut qo = 0u32;
            while qo < sl {
                ws.host_tiles.push(cur);
                ws.host_tiles.push(sl);
                ws.host_tiles.push(qo);
                qo += 64;
            }
            last_indices.push((cur + sl - 1) as usize);
            cur += sl;
        }

        model.gather_token_embeddings(&flat_ids, &mut ws.host_in_f16)?;
        self.api.h2d(ws.d_hidden, &ws.host_in_f16)?;
        self.api.h2d(ws.d_pos, &ws.host_pos)?;
        self.api.h2d(ws.d_tiles, &ws.host_tiles)?;
        let num_tiles = (ws.host_tiles.len() / 3) as u32;
        let total_t_u32 = total_t as u32;

        let api = &self.api;
        let kernels = &self.kernels;
        let ws = &*ws;
        let use_128 = total_t > 64;

        let run_gemm = |a_ptr: CuDevicePtr,
                        w: &Q8Weight,
                        c_ptr: CuDevicePtr,
                        n: u32,
                        k: u32,
                        res_ptr: Option<CuDevicePtr>|
         -> Result<(), String> {
            let n8 = w.n_elems / 8;
            let mut p_qs = w.d_qs;
            let mut p_d = w.d_d;
            let mut p_out = ws.d_w_f16;
            let mut p_n8 = n8;
            let mut deq_args = [
                (&mut p_qs as *mut u64).cast::<c_void>(),
                (&mut p_d as *mut u64).cast::<c_void>(),
                (&mut p_out as *mut u64).cast::<c_void>(),
                (&mut p_n8 as *mut u32).cast::<c_void>(),
            ];
            api.launch(
                kernels.dequant_q8,
                (n8.div_ceil(256), 1, 1),
                (256, 1, 1),
                0,
                &mut deq_args,
            )?;

            let mut p_a = a_ptr;
            let mut p_b = ws.d_w_f16;
            let mut p_c = c_ptr;
            let mut p_m = total_t_u32;
            let mut p_n = n;
            let mut p_k = k;

            let (func, gy) = if use_128 {
                (
                    if res_ptr.is_some() {
                        kernels.gemm_128_res
                    } else {
                        kernels.gemm_128
                    },
                    total_t_u32.div_ceil(128),
                )
            } else {
                (
                    if res_ptr.is_some() {
                        kernels.gemm_64_res
                    } else {
                        kernels.gemm_64
                    },
                    total_t_u32.div_ceil(64),
                )
            };
            let gx = n.div_ceil(128);
            if let Some(r_ptr) = res_ptr {
                let mut p_r = r_ptr;
                let mut args = [
                    (&mut p_a as *mut u64).cast::<c_void>(),
                    (&mut p_b as *mut u64).cast::<c_void>(),
                    (&mut p_c as *mut u64).cast::<c_void>(),
                    (&mut p_r as *mut u64).cast::<c_void>(),
                    (&mut p_m as *mut u32).cast::<c_void>(),
                    (&mut p_n as *mut u32).cast::<c_void>(),
                    (&mut p_k as *mut u32).cast::<c_void>(),
                ];
                api.launch(func, (gx, gy, 1), (256, 1, 1), 0, &mut args)
            } else {
                let mut args = [
                    (&mut p_a as *mut u64).cast::<c_void>(),
                    (&mut p_b as *mut u64).cast::<c_void>(),
                    (&mut p_c as *mut u64).cast::<c_void>(),
                    (&mut p_m as *mut u32).cast::<c_void>(),
                    (&mut p_n as *mut u32).cast::<c_void>(),
                    (&mut p_k as *mut u32).cast::<c_void>(),
                ];
                api.launch(func, (gx, gy, 1), (256, 1, 1), 0, &mut args)
            }
        };

        let mut eps = 1e-6_f32;
        for (l_idx, ly) in model.layers.iter().enumerate() {
            // 1. Attention RMSNorm
            {
                let mut p_x = ws.d_hidden;
                let mut p_w = ly.attn_norm;
                let mut p_o = ws.d_norm;
                let mut args = [
                    (&mut p_x as *mut u64).cast::<c_void>(),
                    (&mut p_w as *mut u64).cast::<c_void>(),
                    (&mut p_o as *mut u64).cast::<c_void>(),
                    (&mut eps as *mut f32).cast::<c_void>(),
                ];
                api.launch(kernels.rmsnorm_1024, (total_t_u32, 1, 1), (256, 1, 1), 0, &mut args)?;
            }
            // 2. Fused QKV GEMM
            run_gemm(ws.d_norm, &ly.qkv, ws.d_big, QKV_DIM as u32, HIDDEN_DIM as u32, None)?;
            // 3. QK-Norm + RoPE in-place
            {
                let mut p_qkv = ws.d_big;
                let mut p_qn = ly.q_norm;
                let mut p_kn = ly.k_norm;
                let mut p_pos = ws.d_pos;
                let mut p_cos = ws.d_cos;
                let mut p_sin = ws.d_sin;
                let mut args = [
                    (&mut p_qkv as *mut u64).cast::<c_void>(),
                    (&mut p_qn as *mut u64).cast::<c_void>(),
                    (&mut p_kn as *mut u64).cast::<c_void>(),
                    (&mut p_pos as *mut u64).cast::<c_void>(),
                    (&mut p_cos as *mut u64).cast::<c_void>(),
                    (&mut p_sin as *mut u64).cast::<c_void>(),
                    (&mut eps as *mut f32).cast::<c_void>(),
                ];
                api.launch(kernels.qk_norm_rope, (total_t_u32, 24, 1), (64, 1, 1), 0, &mut args)?;
            }
            // 3b. Save prefix K/V if prefix pass
            if save_prefix {
                let mut p_qkv = ws.d_big;
                let mut p_k = ws.d_pref_k[l_idx];
                let mut p_v = ws.d_pref_v[l_idx];
                let mut args = [
                    (&mut p_qkv as *mut u64).cast::<c_void>(),
                    (&mut p_k as *mut u64).cast::<c_void>(),
                    (&mut p_v as *mut u64).cast::<c_void>(),
                ];
                api.launch(kernels.copy_prefix_kv, (total_t_u32, 1, 1), (256, 1, 1), 0, &mut args)?;
            }
            // 4. Tensor Core FlashAttention-2
            {
                let mut p_qkv = ws.d_big;
                let mut p_out = ws.d_mid;
                let mut p_tiles = ws.d_tiles;
                let mut p_pk = if use_prefix_len > 0 { ws.d_pref_k[l_idx] } else { 0 };
                let mut p_pv = if use_prefix_len > 0 { ws.d_pref_v[l_idx] } else { 0 };
                let mut p_pl = use_prefix_len;
                let mut args = [
                    (&mut p_qkv as *mut u64).cast::<c_void>(),
                    (&mut p_out as *mut u64).cast::<c_void>(),
                    (&mut p_tiles as *mut u64).cast::<c_void>(),
                    (&mut p_pk as *mut u64).cast::<c_void>(),
                    (&mut p_pv as *mut u64).cast::<c_void>(),
                    (&mut p_pl as *mut u32).cast::<c_void>(),
                ];
                api.launch(kernels.flash_attn_tc, (num_tiles, 16, 1), (128, 1, 1), 0, &mut args)?;
            }
            // 5. Attn Output GEMM + Residual
            run_gemm(
                ws.d_mid,
                &ly.attn_o,
                ws.d_hidden,
                HIDDEN_DIM as u32,
                2048,
                Some(ws.d_hidden),
            )?;
            // 6. FFN RMSNorm
            {
                let mut p_x = ws.d_hidden;
                let mut p_w = ly.ffn_norm;
                let mut p_o = ws.d_norm;
                let mut args = [
                    (&mut p_x as *mut u64).cast::<c_void>(),
                    (&mut p_w as *mut u64).cast::<c_void>(),
                    (&mut p_o as *mut u64).cast::<c_void>(),
                    (&mut eps as *mut f32).cast::<c_void>(),
                ];
                api.launch(kernels.rmsnorm_1024, (total_t_u32, 1, 1), (256, 1, 1), 0, &mut args)?;
            }
            // 7. Fused Gate+Up GEMM
            run_gemm(ws.d_norm, &ly.gate_up, ws.d_big, 6144, HIDDEN_DIM as u32, None)?;
            // 8. Vectorized SwiGLU
            {
                let mut p_gu = ws.d_big;
                let mut p_out = ws.d_mid;
                let mut args = [
                    (&mut p_gu as *mut u64).cast::<c_void>(),
                    (&mut p_out as *mut u64).cast::<c_void>(),
                ];
                api.launch(kernels.swiglu_3072, (total_t_u32, 1, 1), (256, 1, 1), 0, &mut args)?;
            }
            // 9. FFN Down GEMM + Residual
            run_gemm(
                ws.d_mid,
                &ly.ffn_down,
                ws.d_hidden,
                HIDDEN_DIM as u32,
                FFN_DIM as u32,
                Some(ws.d_hidden),
            )?;
        }

        api.sync()?;
        if save_prefix {
            return Ok(Vec::new());
        }

        let mut rows = Vec::with_capacity(last_indices.len());
        for idx in last_indices {
            let mut row = [0u16; HIDDEN_DIM];
            api.d2h(&mut row, ws.d_hidden + (idx as u64) * (HIDDEN_DIM as u64 * 2))?;
            rows.push(row);
        }
        Ok(rows)
    }

    fn reap_idle(&mut self) {
        if self.bind_ctx().is_err() {
            return;
        }
        let api = &self.api;
        self.models.retain(|_, m| {
            if m.idle_release_secs > 0
                && m.last_used.elapsed() > Duration::from_secs(m.idle_release_secs)
            {
                m.free_gpu(api);
                false
            } else {
                true
            }
        });
        if self.models.is_empty()
            && let Some(mut ws) = self.ws.take()
        {
            ws.free_all(api);
        }
    }

    fn shutdown_all(&mut self) {
        if self.bind_ctx().is_err() {
            return;
        }
        let api = &self.api;
        for (_, mut m) in self.models.drain() {
            m.free_gpu(api);
        }
        if let Some(mut ws) = self.ws.take() {
            ws.free_all(api);
        }
    }
}

static CUDA_RUNTIME: OnceLock<Option<Mutex<CudaRuntime>>> = OnceLock::new();

fn runtime() -> Option<&'static Mutex<CudaRuntime>> {
    if std::env::var_os("FOLIO_RAG_DISABLE_CUDA").is_some() {
        return None;
    }
    CUDA_RUNTIME
        .get_or_init(|| CudaRuntime::init().ok().map(Mutex::new))
        .as_ref()
}

/// Checks whether the local GGUF file can be accelerated by the native CUDA Tensor Core engine.
pub fn is_supported_model(model_path: &str, is_reranker: bool) -> bool {
    if model_path.is_empty() {
        return false;
    }
    if is_model_loaded(model_path, is_reranker) {
        return true;
    }
    if runtime().is_none() {
        return false;
    }
    let path = Path::new(model_path);
    if !path.is_file() {
        return false;
    }
    parse_gguf_header(path)
        .and_then(|hdr| validate_qwen3_q8_header(&hdr, is_reranker))
        .is_ok()
}

pub fn is_model_loaded(model_path: &str, is_reranker: bool) -> bool {
    if model_path.is_empty() {
        return false;
    }
    let Some(rt_lock) = CUDA_RUNTIME.get().and_then(|o| o.as_ref()) else {
        return false;
    };
    let key = dunce::canonicalize(model_path).unwrap_or_else(|_| PathBuf::from(model_path));
    let rt = rt_lock.lock().expect("cuda runtime poisoned");
    rt.models
        .get(&key)
        .is_some_and(|m| m.is_reranker == is_reranker)
}

pub fn ensure_ready(model_path: &str, is_reranker: bool, idle_release_secs: u64) -> bool {
    let Some(rt_lock) = runtime() else {
        return false;
    };
    let mut rt = rt_lock.lock().expect("cuda runtime poisoned");
    rt.reap_idle();
    rt.ensure_model_loaded(Path::new(model_path), is_reranker, idle_release_secs)
        .is_ok()
}

pub fn reap_idle_models() {
    if let Some(rt_lock) = CUDA_RUNTIME.get().and_then(|o| o.as_ref()) {
        rt_lock.lock().expect("cuda runtime poisoned").reap_idle();
    }
}

pub fn shutdown_models() {
    if let Some(rt_lock) = CUDA_RUNTIME.get().and_then(|o| o.as_ref()) {
        rt_lock
            .lock()
            .expect("cuda runtime poisoned")
            .shutdown_all();
    }
}

fn encode_plain(text: &str) -> Vec<u32> {
    shared_tokenizer()
        .encode(text, false)
        .expect("Qwen3 tokenizer encodes UTF-8")
        .get_ids()
        .to_vec()
}

fn apply_output_norm_and_l2(raw: &[u16; HIDDEN_DIM], norm_w: &[f32]) -> Vec<f32> {
    let mut vals = [0.0_f32; HIDDEN_DIM];
    let mut ss = 0.0_f64;
    for i in 0..HIDDEN_DIM {
        let v = f16_bits_to_f32(raw[i]);
        vals[i] = v;
        ss += (v as f64) * (v as f64);
    }
    let inv = (1.0_f64 / (ss / HIDDEN_DIM as f64 + 1e-6_f64).sqrt()) as f32;
    let mut l2 = 0.0_f64;
    let mut out = vec![0.0_f32; HIDDEN_DIM];
    for i in 0..HIDDEN_DIM {
        let n = vals[i] * inv * norm_w[i];
        out[i] = n;
        l2 += (n as f64) * (n as f64);
    }
    let inv_l2 = (1.0_f64 / l2.sqrt().max(1e-12)) as f32;
    for v in &mut out {
        *v *= inv_l2;
    }
    out
}

fn apply_output_norm_and_rank(raw: &[u16; HIDDEN_DIM], norm_w: &[f32], cls_w: &[Vec<f32>; 2]) -> f64 {
    let mut vals = [0.0_f32; HIDDEN_DIM];
    let mut ss = 0.0_f64;
    for i in 0..HIDDEN_DIM {
        let v = f16_bits_to_f32(raw[i]);
        vals[i] = v;
        ss += (v as f64) * (v as f64);
    }
    let inv = 1.0_f64 / (ss / HIDDEN_DIM as f64 + 1e-6_f64).sqrt();
    let mut yes = 0.0_f64;
    let mut no = 0.0_f64;
    for i in 0..HIDDEN_DIM {
        let n = (vals[i] as f64) * inv * (norm_w[i] as f64);
        yes += n * (cls_w[0][i] as f64);
        no += n * (cls_w[1][i] as f64);
    }
    let diff = yes - no;
    if diff >= 0.0 {
        1.0 / (1.0 + (-diff).exp())
    } else {
        let e = diff.exp();
        e / (1.0 + e)
    }
}

pub fn embed_texts(
    model_path: &str,
    texts: &[&str],
    is_query: bool,
    idle_release_secs: u64,
) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let rt_lock = runtime().ok_or_else(|| "CUDA 推理引擎不可用".to_owned())?;

    // Tokenize on CPU outside the GPU mutex so tokenization never blocks other callers.
    let max_body = MAX_ROPE_POS.min(MAX_BATCH_TOKENS) - 1;
    let mut seqs: Vec<Vec<u32>> = Vec::with_capacity(texts.len());
    for &t in texts {
        let mut ids = if is_query {
            let full = format!("{QUERY_INSTRUCT}{t}");
            encode_plain(&full)
        } else {
            encode_plain(t)
        };
        if ids.len() > max_body {
            ids.truncate(max_body);
        }
        ids.push(TOKEN_ENDOFTEXT);
        seqs.push(ids);
    }

    let mut rt = rt_lock.lock().expect("cuda runtime poisoned");
    let key = rt.ensure_model_loaded(Path::new(model_path), false, idle_release_secs)?;
    let norm_w = rt.models[&key].output_norm_w.clone();

    let mut results = Vec::with_capacity(seqs.len());
    let mut batch: Vec<&[u32]> = Vec::new();
    let mut batch_tokens = 0usize;

    let flush = |rt: &mut CudaRuntime,
                 batch: &mut Vec<&[u32]>,
                 results: &mut Vec<Vec<f32>>|
     -> Result<(), String> {
        if batch.is_empty() {
            return Ok(());
        }
        let rows = rt.forward_packed(&key, batch, false, 0)?;
        for row in &rows {
            results.push(apply_output_norm_and_l2(row, &norm_w));
        }
        batch.clear();
        Ok(())
    };

    for s in &seqs {
        if !batch.is_empty() && batch_tokens + s.len() > MAX_BATCH_TOKENS {
            flush(&mut rt, &mut batch, &mut results)?;
            batch_tokens = 0;
        }
        batch.push(s.as_slice());
        batch_tokens += s.len();
    }
    flush(&mut rt, &mut batch, &mut results)?;
    Ok(results)
}

fn build_rerank_prefix_tokens(query: &str) -> Vec<u32> {
    static STATIC_HEAD: OnceLock<Vec<u32>> = OnceLock::new();
    static STATIC_DOC_TAG: OnceLock<Vec<u32>> = OnceLock::new();
    let head = STATIC_HEAD.get_or_init(|| {
        let mut v = Vec::with_capacity(64);
        v.push(TOKEN_IM_START);
        v.extend_from_slice(&encode_plain(
            "system\nJudge whether the Document meets the requirements based on the Query and the Instruct provided. Note that the answer can only be \"yes\" or \"no\".",
        ));
        v.push(TOKEN_IM_END);
        v.extend_from_slice(&encode_plain("\n"));
        v.push(TOKEN_IM_START);
        v.extend_from_slice(&encode_plain(
            "user\n<Instruct>: Given a web search query, retrieve relevant passages that answer the query\n<Query>: ",
        ));
        v
    });
    let doc_tag = STATIC_DOC_TAG.get_or_init(|| encode_plain("\n<Document>: "));

    let mut q_ids = encode_plain(query);
    let max_q = MAX_PREFIX_TOKENS.saturating_sub(head.len() + doc_tag.len());
    if q_ids.len() > max_q {
        q_ids.truncate(max_q);
    }
    let mut pref = Vec::with_capacity(head.len() + q_ids.len() + doc_tag.len());
    pref.extend_from_slice(head);
    pref.extend_from_slice(&q_ids);
    pref.extend_from_slice(doc_tag);
    pref
}

fn rerank_suffix_tail_tokens() -> &'static [u32] {
    static TAIL: OnceLock<Vec<u32>> = OnceLock::new();
    TAIL.get_or_init(|| {
        let mut v = Vec::with_capacity(12);
        v.push(TOKEN_IM_END);
        v.extend_from_slice(&encode_plain("\n"));
        v.push(TOKEN_IM_START);
        v.extend_from_slice(&encode_plain("assistant\n"));
        v.push(TOKEN_THINK_OPEN);
        v.extend_from_slice(&encode_plain("\n\n"));
        v.push(TOKEN_THINK_CLOSE);
        v.extend_from_slice(&encode_plain("\n\n"));
        v
    })
}

pub fn rerank_documents(
    model_path: &str,
    query: &str,
    documents: &[&str],
    idle_release_secs: u64,
) -> Result<Vec<f64>, String> {
    if documents.is_empty() {
        return Ok(Vec::new());
    }
    let rt_lock = runtime().ok_or_else(|| "CUDA 推理引擎不可用".to_owned())?;

    let prefix_ids = build_rerank_prefix_tokens(query);
    let tail = rerank_suffix_tail_tokens();
    let max_doc_body = (MAX_ROPE_POS - prefix_ids.len())
        .min(MAX_BATCH_TOKENS)
        .saturating_sub(tail.len());

    let mut doc_seqs: Vec<Vec<u32>> = Vec::with_capacity(documents.len());
    for &doc in documents {
        let mut ids = encode_plain(doc);
        if ids.len() > max_doc_body {
            ids.truncate(max_doc_body);
        }
        ids.extend_from_slice(tail);
        doc_seqs.push(ids);
    }

    let mut rt = rt_lock.lock().expect("cuda runtime poisoned");
    let key = rt.ensure_model_loaded(Path::new(model_path), true, idle_release_secs)?;
    let norm_w = rt.models[&key].output_norm_w.clone();
    let cls_w = rt.models[&key]
        .cls_w
        .clone()
        .ok_or_else(|| "Reranker 缺少分类头权重".to_owned())?;

    // 1. Compute shared prefix K/V ONCE across all 28 layers!
    let pref_slice = [prefix_ids.as_slice()];
    rt.forward_packed(&key, &pref_slice, true, 0)?;
    let prefix_len = prefix_ids.len() as u32;

    // 2. Score all documents in mega-batches attending to the shared prefix K/V!
    let mut scores = Vec::with_capacity(doc_seqs.len());
    let mut batch: Vec<&[u32]> = Vec::new();
    let mut batch_tokens = 0usize;

    let flush = |rt: &mut CudaRuntime,
                 batch: &mut Vec<&[u32]>,
                 scores: &mut Vec<f64>|
     -> Result<(), String> {
        if batch.is_empty() {
            return Ok(());
        }
        let rows = rt.forward_packed(&key, batch, false, prefix_len)?;
        for row in &rows {
            scores.push(apply_output_norm_and_rank(row, &norm_w, &cls_w));
        }
        batch.clear();
        Ok(())
    };

    for s in &doc_seqs {
        if !batch.is_empty() && batch_tokens + s.len() > MAX_BATCH_TOKENS {
            flush(&mut rt, &mut batch, &mut scores)?;
            batch_tokens = 0;
        }
        batch.push(s.as_slice());
        batch_tokens += s.len();
    }
    flush(&mut rt, &mut batch, &mut scores)?;
    Ok(scores)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_roundtrip_sanity() {
        for v in [0.0_f32, 1.0, -1.0, 0.08838835, 3.140625, -42.5, 1e-4] {
            let bits = f32_to_f16_bits(v);
            let back = f16_bits_to_f32(bits);
            assert!((v - back).abs() <= v.abs() * 1e-3 + 1e-5, "v={v}, back={back}");
        }
    }

    #[test]
    #[ignore = "requires NVIDIA RTX SM>=8.0 GPU and local Qwen3-0.6B GGUF models"]
    fn benchmark_native_cuda_vs_baseline_ref() {
        let embed_path = std::env::var("FOLIO_LIVE_EMBED_MODEL")
            .unwrap_or_else(|_| "D:/Models/folio-rag/Qwen3-Embedding-0.6B-Q8_0.gguf".into());
        let rerank_path = std::env::var("FOLIO_LIVE_RERANK_MODEL")
            .unwrap_or_else(|_| "D:/Models/folio-rag/qwen3-reranker-0.6b-q8_0.gguf".into());

        let gpu_mem = || -> String {
            std::process::Command::new("nvidia-smi")
                .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .unwrap_or_default()
        };
        let vram_before = gpu_mem();

        // Cold load timings (includes CUDA init + PTX JIT on first model)
        let t0 = Instant::now();
        {
            let rt_lock = runtime().expect("runtime() returned None");
            rt_lock
                .lock()
                .unwrap()
                .ensure_model_loaded(Path::new(&embed_path), false, 300)
                .expect("load embed model failed");
        }
        let embed_cold_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let vram_embed = gpu_mem();

        let t0 = Instant::now();
        {
            let rt_lock = runtime().expect("runtime() returned None");
            rt_lock
                .lock()
                .unwrap()
                .ensure_model_loaded(Path::new(&rerank_path), true, 300)
                .expect("load rerank model failed");
        }
        let rerank_cold_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let vram_both = gpu_mem();

        // Optional exact parity check against scratch baseline_ref.json if provided
        if let Ok(ref_path) = std::env::var("FOLIO_BENCH_REF_JSON") {
            let raw = std::fs::read_to_string(&ref_path).expect("read baseline_ref.json");
            let val: serde_json::Value = serde_json::from_str(&raw).expect("parse json");
            let chunks: Vec<String> = val["chunks_64"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            let chunk_refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
            let rerank_q = val["rerank_query"].as_str().unwrap();

            // Query embedding (warmup + 5 runs)
            let q_text = rerank_q;
            let qv = embed_texts(&embed_path, &[q_text], true, 300).unwrap();
            let ref_qv: Vec<f32> = val["query_embedding"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect();
            let cos_q = cosine(&qv[0], &ref_qv);
            let mut q_min = f64::MAX;
            for _ in 0..5 {
                let t = Instant::now();
                let _ = embed_texts(&embed_path, &[q_text], true, 300).unwrap();
                q_min = q_min.min(t.elapsed().as_secs_f64() * 1000.0);
            }

            // 64 chunks embedding (warmup + 2 runs)
            let vecs = embed_texts(&embed_path, &chunk_refs, false, 300).unwrap();
            let mut min_cos8 = 1.0_f64;
            for (i, ref_arr) in val["chunk_embeddings_first8"].as_array().unwrap().iter().enumerate() {
                let rv: Vec<f32> = ref_arr.as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
                min_cos8 = min_cos8.min(cosine(&vecs[i], &rv));
            }
            let t = Instant::now();
            let _ = embed_texts(&embed_path, &chunk_refs, false, 300).unwrap();
            let embed_64_ms = t.elapsed().as_secs_f64() * 1000.0;

            // 25 docs rerank (warmup + 3 runs)
            let docs25 = &chunk_refs[..25];
            let scores = rerank_documents(&rerank_path, rerank_q, docs25, 300).unwrap();
            let ref_scores: Vec<f64> = val["rerank_scores_25"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            let max_diff = scores
                .iter()
                .zip(&ref_scores)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f64, f64::max);
            let mut rr_min = f64::MAX;
            for _ in 0..3 {
                let t = Instant::now();
                let _ = rerank_documents(&rerank_path, rerank_q, docs25, 300).unwrap();
                rr_min = rr_min.min(t.elapsed().as_secs_f64() * 1000.0);
            }

            println!("=== NATIVE RUST CUDA BENCHMARK (RTX 5060) ===");
            println!("VRAM: before={vram_before} MiB | +Embed={vram_embed} MiB | +Both={vram_both} MiB");
            println!("Embed cold load:   {embed_cold_ms:.1} ms");
            println!("Rerank cold load:  {rerank_cold_ms:.1} ms");
            println!("Embed query (min): {q_min:.2} ms (cos={cos_q:.6})");
            println!("Embed 64 chunks:   {embed_64_ms:.1} ms ({:.1} chunks/s, min_cos8={min_cos8:.6})", 64_000.0 / embed_64_ms);
            println!("Rerank 25 docs:    {rr_min:.1} ms (max_score_diff={max_diff:.5})");
            assert!(cos_q > 0.999, "query cosine {cos_q} < 0.999");
            assert!(min_cos8 > 0.999, "chunk cosine {min_cos8} < 0.999");
            assert!(max_diff < 0.02, "rerank max_diff {max_diff} >= 0.02");
        }
        shutdown_models();
        println!("VRAM after shutdown: {} MiB", gpu_mem());
    }

    fn cosine(a: &[f32], b: &[f32]) -> f64 {
        let dot: f64 = a.iter().zip(b).map(|(x, y)| (*x as f64) * (*y as f64)).sum();
        let na: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
        let nb: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
        dot / (na * nb).max(1e-12)
    }
}
