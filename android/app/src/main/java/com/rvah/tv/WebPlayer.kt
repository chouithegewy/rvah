package com.rvah.tv

import android.annotation.SuppressLint
import android.content.Context
import android.os.Message
import android.os.SystemClock
import android.util.Log
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.GeolocationPermissions
import android.webkit.JsPromptResult
import android.webkit.JsResult
import android.webkit.PermissionRequest
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import java.io.ByteArrayInputStream
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.rvah_android.AdBlocker

/** How long a web video waits for the ad blocker before it loads without one. */
private const val BLOCKER_WAIT_MS = 5_000L

/**
 * A web source, full screen: the page at [url] and nothing else. It cannot
 * open windows or leave its host, ads and trackers are dropped (except hosts
 * in [neverBlock]), OK taps the middle of the page (most embedded players
 * start on a click), and Back leaves.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
internal fun WebPlayerScreen(url: String, neverBlock: List<String>, onExit: () -> Unit) {
    BackHandler(onBack = onExit)
    val context = LocalContext.current
    val view = LocalView.current
    var ready by remember(url) { mutableStateOf(false) }
    var failed by remember(url) { mutableStateOf(false) }
    // Read on WebView's request threads, so not Compose state.
    val blocker = remember(url) { AtomicReference<AdBlocker?>(null) }
    val frame = remember { arrayOfNulls<WebPlayerFrame>(1) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(url) {
        blocker.set(withTimeoutOrNull(BLOCKER_WAIT_MS) { FilterLists.blocker(context) })
        ready = true
    }
    DisposableEffect(view) {
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = false }
    }
    // Leaving the app (the TV's Home button, say) closes the player, which
    // ends its session and silences the page, rather than leaving it
    // playing unseen.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_STOP) onExit() }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    if (failed) {
        Page("This server isn't working") { Body("Try another server.", ErrorText) }
        return
    }
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        AndroidView(
            factory = { ctx ->
                WebPlayerFrame(ctx, onExit).also { player ->
                    frame[0] = player
                    player.web.webViewClient = lockedClient(url, neverBlock, blocker) { failed = true }
                }
            },
            update = { player -> if (ready && player.web.url == null) player.web.loadUrl(url) },
            modifier = Modifier.fillMaxSize().focusRequester(focus),
        )
    }
    LaunchedEffect(Unit) {
        runCatching { focus.requestFocus() }
        frame[0]?.web?.requestFocus()
    }
    DisposableEffect(Unit) {
        onDispose {
            frame[0]?.web?.apply {
                stopLoading()
                destroy()
            }
            frame[0] = null
        }
    }
}

/** Requests stay on the page's host; blocked ones get an empty answer. */
private fun lockedClient(
    url: String,
    neverBlock: List<String>,
    blocker: AtomicReference<AdBlocker?>,
    onMainFrameFailed: () -> Unit,
) = object : WebViewClient() {
    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
        val allowed = navigationAllowed(url, request.url.toString(), request.isForMainFrame)
        if (!allowed) Log.i("rvah", "web player: refused navigation to ${request.url.host}")
        return !allowed
    }

    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
        if (request.isForMainFrame) return null
        val target = request.url.toString()
        val host = request.url.host ?: return null
        if (neverBlocked(host, neverBlock)) return null
        val accept = request.requestHeaders.entries.firstOrNull { it.key.equals("Accept", ignoreCase = true) }?.value
        val block = blocker.get()?.shouldBlock(target, url, requestType(target, false, accept)) ?: false
        if (!block) return null
        Log.d("rvah", "web player: blocked $host")
        return WebResourceResponse("text/plain", "utf-8", 204, "No Content", emptyMap(), ByteArrayInputStream(ByteArray(0)))
    }

    override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
        if (request.isForMainFrame) onMainFrameFailed()
    }

    override fun onReceivedHttpError(view: WebView, request: WebResourceRequest, response: WebResourceResponse) {
        if (request.isForMainFrame && response.statusCode >= 400) onMainFrameFailed()
    }
}

/**
 * The WebView in a frame that takes the remote's keys first: OK, Enter and
 * Play/Pause tap the middle of the page, Back leaves fullscreen and then the
 * player, and the arrows go nowhere. A page's own fullscreen view is shown
 * over the WebView.
 */
@SuppressLint("SetJavaScriptEnabled", "ViewConstructor")
private class WebPlayerFrame(context: Context, private val onExit: () -> Unit) : FrameLayout(context) {
    val web = WebView(context)
    private var custom: View? = null
    private var customCallback: WebChromeClient.CustomViewCallback? = null

    init {
        setBackgroundColor(android.graphics.Color.BLACK)
        web.setBackgroundColor(android.graphics.Color.BLACK)
        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
        web.settings.apply {
            javaScriptEnabled = true
            domStorageEnabled = true
            mediaPlaybackRequiresUserGesture = false
            javaScriptCanOpenWindowsAutomatically = false
            setSupportMultipleWindows(true)
            allowFileAccess = false
            allowContentAccess = false
        }
        CookieManager.getInstance().setAcceptThirdPartyCookies(web, true)
        web.setDownloadListener { _, _, _, _, _ -> }
        web.webChromeClient = object : WebChromeClient() {
            override fun onCreateWindow(view: WebView, isDialog: Boolean, isUserGesture: Boolean, resultMsg: Message): Boolean {
                Log.i("rvah", "web player: refused a new window")
                return false
            }

            override fun onShowCustomView(view: View, callback: CustomViewCallback) {
                hideCustom()
                custom = view
                customCallback = callback
                addView(view, LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
            }

            override fun onHideCustomView() {
                custom?.let { removeView(it) }
                custom = null
                customCallback = null
            }

            override fun onPermissionRequest(request: PermissionRequest) = request.deny()

            // A page's alert, confirm or prompt would put a dialog over the
            // video and take the remote's OK: every one is answered "cancel".
            override fun onJsAlert(view: WebView?, url: String?, message: String?, result: JsResult?): Boolean {
                result?.cancel()
                return true
            }

            override fun onJsConfirm(view: WebView?, url: String?, message: String?, result: JsResult?): Boolean {
                result?.cancel()
                return true
            }

            override fun onJsPrompt(
                view: WebView?,
                url: String?,
                message: String?,
                defaultValue: String?,
                result: JsPromptResult?,
            ): Boolean {
                result?.cancel()
                return true
            }

            override fun onJsBeforeUnload(view: WebView?, url: String?, message: String?, result: JsResult?): Boolean {
                result?.cancel()
                return true
            }

            override fun onGeolocationPermissionsShowPrompt(origin: String, callback: GeolocationPermissions.Callback) =
                callback.invoke(origin, false, false)
        }
        isFocusable = true
        web.isFocusable = true
        addView(web, LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
    }

    /** Leave the page's fullscreen view, telling the page. */
    private fun hideCustom() {
        val shown = custom ?: return
        removeView(shown)
        custom = null
        customCallback?.onCustomViewHidden()
        customCallback = null
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        when (event.keyCode) {
            KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER,
            KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE -> if (event.action == KeyEvent.ACTION_UP) tapCentre(custom ?: web)
            KeyEvent.KEYCODE_BACK -> if (event.action == KeyEvent.ACTION_UP) {
                if (custom != null) hideCustom() else onExit()
            }
            KeyEvent.KEYCODE_DPAD_UP, KeyEvent.KEYCODE_DPAD_DOWN, KeyEvent.KEYCODE_DPAD_LEFT,
            KeyEvent.KEYCODE_DPAD_RIGHT -> Unit
            else -> return super.dispatchKeyEvent(event)
        }
        return true
    }
}

/** A finger tap (down, then up) at the middle of [target]: it reaches players inside cross-site iframes, which page scripts cannot. */
private fun tapCentre(target: View) {
    val x = target.width / 2f
    val y = target.height / 2f
    val down = SystemClock.uptimeMillis()
    for ((action, at) in listOf(MotionEvent.ACTION_DOWN to down, MotionEvent.ACTION_UP to down + 50)) {
        val event = MotionEvent.obtain(down, at, action, x, y, 0)
        event.source = InputDevice.SOURCE_TOUCHSCREEN
        target.dispatchTouchEvent(event)
        event.recycle()
    }
}
