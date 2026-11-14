package app.folio.mobile;

import android.app.Activity;
import android.content.ClipData;
import android.content.Intent;
import android.content.res.Configuration;
import android.database.Cursor;
import android.graphics.Color;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.webkit.JavascriptInterface;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.FrameLayout;
import android.widget.Toast;
import androidx.core.content.FileProvider;
import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.webkit.WebViewAssetLoader;
import org.json.JSONObject;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** Android owns document permissions; Markdown editing and drafts live in the
 * bundled editor. Imports always create a draft and exports use the system picker.
 */
public class MainActivity extends Activity {
    private static final String ORIGIN = "https://appassets.androidplatform.net";
    private static final String ENTRY = ORIGIN + "/assets/index.html";
    private static final int OPEN = 10, SAVE = 11;
    private final ExecutorService io = Executors.newSingleThreadExecutor();
    private WebView web;
    private FrameLayout frame;
    private volatile boolean ready;
    private JSONObject pendingImport;
    private String exportText;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        frame = new FrameLayout(this);
        frame.setBackgroundColor(Color.rgb(250, 250, 248));
        web = new WebView(this);
        frame.addView(web, new FrameLayout.LayoutParams(-1, -1));
        setContentView(frame);
        updateSystemColors();
        // The native view consumes bars/cutouts/IME exactly once. Web CSS viewport
        // then follows the remaining window, including Honor gesture navigation.
        ViewCompat.setOnApplyWindowInsetsListener(frame, (view, windowInsets) -> {
            Insets bars = windowInsets.getInsets(WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
            Insets keyboard = windowInsets.getInsets(WindowInsetsCompat.Type.ime());
            view.setPadding(bars.left, bars.top, bars.right, Math.max(bars.bottom, keyboard.bottom));
            return WindowInsetsCompat.CONSUMED;
        });
        WebSettings settings = web.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        settings.setTextZoom(Math.round(getResources().getConfiguration().fontScale * 100));
        WebViewAssetLoader loader = new WebViewAssetLoader.Builder()
            .addPathHandler("/assets/", new WebViewAssetLoader.AssetsPathHandler(this)).build();
        web.addJavascriptInterface(new DocumentBridge(), "FolioNative");
        web.setWebViewClient(new WebViewClient() {
            @Override public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
                Uri uri = request.getUrl();
                if (isLocal(uri)) return loader.shouldInterceptRequest(uri);
                // Imported Markdown cannot load remote scripts or make requests.
                // Math and fonts are packaged; the application needs no INTERNET permission.
                return new WebResourceResponse("text/plain", "UTF-8", new ByteArrayInputStream(new byte[0]));
            }
            @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                Uri uri = request.getUrl();
                if (isLocal(uri) && uri.getPath().equals("/assets/index.html")) return false;
                if (request.isForMainFrame() && ("https".equals(uri.getScheme()) || "http".equals(uri.getScheme()))) {
                    try { startActivity(new Intent(Intent.ACTION_VIEW, uri)); }
                    catch (Exception e) { error("没有可用的浏览器"); }
                }
                return true;
            }
            @Override public void onPageFinished(WebView view, String url) {
                ready = ENTRY.equals(url);
                if (ready) deliverImport();
            }
        });
        web.loadUrl(ENTRY);
        acceptIntent(getIntent());
    }

    private static boolean isLocal(Uri uri) {
        return "https".equals(uri.getScheme()) && "appassets.androidplatform.net".equals(uri.getHost()) && uri.getPath() != null && uri.getPath().startsWith("/assets/");
    }

    private void updateSystemColors() {
        boolean dark = (getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES;
        int background = dark ? Color.rgb(32, 35, 36) : Color.rgb(250, 250, 248);
        frame.setBackgroundColor(background);
        web.setBackgroundColor(background);
        getWindow().getDecorView().setBackgroundColor(background);
        getWindow().setStatusBarColor(background);
        getWindow().setNavigationBarColor(background);
        WindowCompat.getInsetsController(getWindow(), getWindow().getDecorView()).setAppearanceLightStatusBars(!dark);
        WindowCompat.getInsetsController(getWindow(), getWindow().getDecorView()).setAppearanceLightNavigationBars(!dark);
    }

    @Override public void onConfigurationChanged(Configuration configuration) {
        super.onConfigurationChanged(configuration);
        updateSystemColors();
        web.getSettings().setTextZoom(Math.round(configuration.fontScale * 100));
        ViewCompat.requestApplyInsets(web);
    }

    private void error(String message) {
        runOnUiThread(() -> {
            Toast.makeText(this, message, Toast.LENGTH_LONG).show();
            if (ready) web.evaluateJavascript("window.folioNativeError?.(" + JSONObject.quote(message) + ")", null);
        });
    }

    private void deliverImport() {
        if (ready && pendingImport != null) {
            web.evaluateJavascript("window.folioOpenDocument(" + pendingImport + ")", null);
            pendingImport = null;
        }
    }

    private void importText(String name, String markdown) {
        runOnUiThread(() -> {
            try {
                pendingImport = new JSONObject().put("name", name).put("markdown", markdown);
                deliverImport();
            } catch (Exception e) { error("无法导入文档"); }
        });
    }

    private void importUri(Uri uri) {
        io.execute(() -> {
            try {
                String name = "导入笔记.md";
                try (Cursor cursor = getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
                    if (cursor != null && cursor.moveToFirst()) name = cursor.getString(0);
                }
                try (InputStream input = getContentResolver().openInputStream(uri); ByteArrayOutputStream output = new ByteArrayOutputStream()) {
                    if (input == null) throw new java.io.IOException("No input");
                    byte[] buffer = new byte[8192]; int length;
                    while ((length = input.read(buffer)) != -1) output.write(buffer, 0, length);
                    String text = output.toString(StandardCharsets.UTF_8.name());
                    importText(name, text.startsWith("\uFEFF") ? text.substring(1) : text);
                }
            } catch (Exception e) { error("无法读取文件，请重新选择 Markdown 文档"); }
        });
    }

    private void acceptIntent(Intent intent) {
        if (Intent.ACTION_SEND.equals(intent.getAction())) {
            Uri uri = intent.getParcelableExtra(Intent.EXTRA_STREAM);
            if (uri != null) importUri(uri);
            else if (intent.getCharSequenceExtra(Intent.EXTRA_TEXT) != null) importText("分享的笔记.md", intent.getCharSequenceExtra(Intent.EXTRA_TEXT).toString());
        } else if (Intent.ACTION_VIEW.equals(intent.getAction()) && intent.getData() != null) importUri(intent.getData());
    }

    @Override protected void onNewIntent(Intent intent) { super.onNewIntent(intent); setIntent(intent); acceptIntent(intent); }

    /** The bridge is reachable only from packaged content. HTML input is parsed
     * as Markdown text and all navigation away from the trusted origin is blocked.
     */
    public final class DocumentBridge {
        @JavascriptInterface public void openDocument() {
            runOnUiThread(() -> {
                if (!ready) return;
                Intent picker = new Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*");
                picker.putExtra(Intent.EXTRA_MIME_TYPES, new String[]{"text/*", "application/octet-stream"});
                startActivityForResult(picker, OPEN);
            });
        }
        @JavascriptInterface public void saveDocument(String name, String markdown) {
            runOnUiThread(() -> {
                if (!ready) return;
                exportText = markdown;
                Intent picker = new Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("text/markdown");
                picker.putExtra(Intent.EXTRA_TITLE, safeName(name));
                startActivityForResult(picker, SAVE);
            });
        }
        @JavascriptInterface public void shareDocument(String name, String markdown) {
            if (!ready) return;
            io.execute(() -> {
                try {
                    File directory = new File(getCacheDir(), "shared");
                    if (!directory.exists() && !directory.mkdirs()) throw new java.io.IOException("Cannot create share directory");
                    File file = new File(directory, safeName(name));
                    try (OutputStream output = new FileOutputStream(file)) { output.write(markdown.getBytes(StandardCharsets.UTF_8)); }
                    Uri uri = FileProvider.getUriForFile(MainActivity.this, getPackageName() + ".files", file);
                    runOnUiThread(() -> {
                        Intent share = new Intent(Intent.ACTION_SEND).setType("text/markdown").putExtra(Intent.EXTRA_STREAM, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                        share.setClipData(ClipData.newRawUri("Markdown", uri));
                        startActivity(Intent.createChooser(share, "分享 Markdown"));
                    });
                } catch (Exception e) { error("无法分享文档，请使用导出"); }
            });
        }
    }

    private static String safeName(String name) {
        String clean = name.replaceAll("[\\\\/:*?\"<>|\\p{Cntrl}]", "_");
        if (clean.trim().isEmpty() || clean.equals(".") || clean.equals("..")) clean = "未命名.md";
        return clean;
    }

    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request == SAVE && result != RESULT_OK) { exportText = null; return; }
        if (result != RESULT_OK || data == null || data.getData() == null) return;
        if (request == OPEN) importUri(data.getData());
        if (request == SAVE && exportText != null) {
            String markdown = exportText; exportText = null;
            Uri destination = data.getData();
            io.execute(() -> {
                try (OutputStream output = getContentResolver().openOutputStream(destination, "wt")) {
                    if (output == null) throw new java.io.IOException("No output");
                    output.write(markdown.getBytes(StandardCharsets.UTF_8));
                    runOnUiThread(() -> web.evaluateJavascript("window.folioNativeSaved?.()", null));
                } catch (Exception e) { error("导出失败，请重新选择保存位置"); }
            });
        }
    }

    @Override public void onBackPressed() {
        web.evaluateJavascript("(()=>{const d=document.querySelector('dialog[open]');if(d){d.close();return true;}return false;})()", handled -> {
            if (!"true".equals(handled)) MainActivity.super.onBackPressed();
        });
    }
    @Override protected void onDestroy() {
        web.removeJavascriptInterface("FolioNative"); web.destroy(); io.shutdown();
        super.onDestroy();
    }
}
