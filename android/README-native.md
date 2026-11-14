# Android 构建与文件接口

要求 JDK 17、Gradle 8.11.1、Android SDK platform 35 / build-tools 35.0.0、Node.js。Android Gradle Plugin 固定为 8.9.1。

本次本机构建工具放在 `C:\Users\12732\.cache\folio-android`，不在仓库内。重建：

```powershell
.\scripts\build-android.ps1
```

本机 Google Maven 原始地址出现 TLS/超时错误，构建脚本默认使用阿里云 Google Maven 镜像；可传 `-GoogleMaven 'https://dl.google.com/android/maven2'` 改回官方源。运行 APK 不依赖任何 Maven 源。

其他电脑可安装 Android Studio 的 SDK，再指定现有工具路径：

```powershell
.\scripts\build-android.ps1 -SdkPath 'C:\Android\Sdk' -GradlePath 'C:\tools\gradle-8.11.1\bin\gradle.bat' -JavaPath 'C:\Java\jdk-17'
```

产物：`dist/android/Folio-Android-0.1.0.apk`。APK 使用本机 Android debug 签名，供直接安装试用；后续更新需沿用相同签名。最低 Android 8.0，目标 API 35，使用系统 Android WebView，无额外 ABI/native-library 限制。

## 打包

前端 `android/web/dist` 整体作为 APK assets，入口是 `https://appassets.androidplatform.net/assets/index.html`。WebViewAssetLoader 提供固定本地 HTTPS origin，字体、JS 和 CSS 无需联网。原生无 INTERNET 和全盘存储权限。

外壳消费系统栏、刘海及输入法 Insets，再把可用空间交给 WebView；字号按系统 fontScale 调整。屏幕宽高和 DPI 交给 WebView viewport 换算，支持用户提供的荣耀 GT Pro 1264 × 2800 物理屏幕参数。

## 桥接协议

| 方向 | 接口 | 行为 |
| --- | --- | --- |
| Web → Android | `FolioNative.openDocument()` | 系统 ACTION_OPEN_DOCUMENT 选择文档 |
| Web → Android | `FolioNative.saveDocument(name, markdown)` | ACTION_CREATE_DOCUMENT 导出 UTF-8 文件 |
| Web → Android | `FolioNative.shareDocument(name, markdown)` | 通过 FileProvider 和临时只读 URI 分享 |
| Android → Web | `folioOpenDocument({name, markdown})` | 创建本机笔记副本 |
| Android → Web | `folioNativeError(message)` | 展示文件操作失败信息 |
| Android → Web | `folioNativeSaved()` | 写入完成后展示成功反馈 |

导入和导出使用单个系统授权的 URI，不修改导入文件。磁盘读写在单独线程执行，导出只在实际写入成功后通知前端。应用还可接收其他 Android 应用发送的 Markdown 文件或纯文本。

不允许 WebView 导航到外部页面，阅读模式下的网页链接交给系统浏览器。Markdown HTML 默认作为文本解析，KaTeX `trust=false`；外部子资源请求被拦截，本地桥只供打包的编辑器使用。
