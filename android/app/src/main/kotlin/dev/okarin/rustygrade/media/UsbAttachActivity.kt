package dev.okarin.rustygrade.media

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/**
 * Invisible target of the manifest's USB_DEVICE_ATTACHED filter. Being the registered handler for
 * USB audio devices is what makes Android offer "Always open" in its USB dialogs; once the user ticks
 * it, the system grants the USB permission itself on every later plug-in, with no prompt.
 *
 * It shows nothing: it pokes the running playback service (if any) to re-check the DAC now that the
 * permission is in place, then finishes immediately.
 */
class UsbAttachActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        sendBroadcast(Intent(UsbDacController.attachHandledAction(this)).setPackage(packageName))
        finish()
    }
}
