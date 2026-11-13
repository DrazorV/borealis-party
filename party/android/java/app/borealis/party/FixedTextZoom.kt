package app.borealis.party

import android.app.Activity
import android.app.Application
import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.net.Uri
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView

/**
 * Keeps the app's web view at its own text size (100 %), whatever Android's font size is.
 *
 * An Android WebView starts with `textZoom` = the system font scale, which scales every font size and every fixed
 * line height on the page, but no box, border or background. On the character sheet preview the text then grew past
 * the ruled lines it is written on and pushed page 1's lists past the page footer (a phone with a larger font size).
 * The party UI is laid out in CSS pixels for the phone sizes it is tested at, as on iOS and the desktop, where the
 * system text size doesn't reach the page either.
 *
 * Upstream's activity stays as it is (party.init.gradle adds this file and its provider): the provider only registers
 * [FixedTextZoom] when the app starts, which finds the WebView once Tauri has added it.
 */
class PartyStartup : ContentProvider() {
    override fun onCreate(): Boolean {
        (context?.applicationContext as? Application)?.registerActivityLifecycleCallbacks(FixedTextZoom)
        return true
    }

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, args: Array<out String>?, sort: String?): Cursor? = null
    override fun getType(uri: Uri): String? = null
    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, selection: String?, args: Array<out String>?): Int = 0
    override fun update(uri: Uri, values: ContentValues?, selection: String?, args: Array<out String>?): Int = 0
}

object FixedTextZoom : Application.ActivityLifecycleCallbacks {
    const val ZOOM = 100

    /** Sets [ZOOM] on every WebView under [view]; answers how many it changed. */
    fun apply(view: View): Int = when (view) {
        is WebView -> if (view.settings.textZoom != ZOOM) { view.settings.textZoom = ZOOM; 1 } else 0
        is ViewGroup -> (0 until view.childCount).sumOf { apply(view.getChildAt(it)) }
        else -> 0
    }

    override fun onActivityCreated(activity: Activity, state: Bundle?) {
        // The WebView is added after onCreate; each layout pass checks again (a cheap walk of a few views).
        val root = activity.window.decorView
        root.viewTreeObserver.addOnGlobalLayoutListener { apply(root) }
    }

    override fun onActivityStarted(activity: Activity) {}
    override fun onActivityResumed(activity: Activity) {}
    override fun onActivityPaused(activity: Activity) {}
    override fun onActivityStopped(activity: Activity) {}
    override fun onActivitySaveInstanceState(activity: Activity, state: Bundle) {}
    override fun onActivityDestroyed(activity: Activity) {}
}
