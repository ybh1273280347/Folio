/** Folio compiles TeX sources via its native latexmk pipeline; the SDK only transports requests. */
import { Type } from '@sinclair/typebox';

const text = Type.String({ minLength: 1 });
const engine = Type.Union(['xelatex', 'pdflatex', 'lualatex'].map(value => Type.Literal(value)));

export function compileTools(request) {
  return [
    {
      name: 'folio_tex_compile',
      label: '批量编译 TeX 为 PDF',
      description:
        '使用 Folio 内置的 latexmk 管线将一个或多个 .tex 文件批量编译为同名 PDF（输出到源文件同目录）。在 Folio 项目中编译 TeX 文档时应优先使用此工具，避免通过 bash/powershell 直接调用 xelatex、pdflatex、lualatex 或 latexmk CLI。必须在 paths 和 under_directory 中二选一：paths 指定具体 .tex 文件相对路径；under_directory 递归编译目录下所有 .tex 文件（"." 表示当前空间根目录）。engine 可选，省略时沿用当前项目设置。',
      parameters: Type.Object(
        {
          paths: Type.Optional(
            Type.Array(text, {
              minItems: 1,
              maxItems: 50,
              description: '仅接受具体的 .tex 文件相对路径，不接受目录或通配符。编译目录请使用 under_directory。',
            }),
          ),
          under_directory: Type.Optional(
            Type.String({
              minLength: 1,
              description: '当前项目或默认空间内的目录相对路径，递归查找并编译其中所有 .tex 文件；"." 表示根目录。',
            }),
          ),
          engine: Type.Optional(engine),
        },
        { additionalProperties: false },
      ),
      execute: async (_id, params, signal) => {
        const result = await request('compile_tex', params, signal);
        return {
          content: [{ type: 'text', text: JSON.stringify(result) }],
          details: result,
          isError: result.status !== 'ok',
        };
      },
    },
  ];
}
