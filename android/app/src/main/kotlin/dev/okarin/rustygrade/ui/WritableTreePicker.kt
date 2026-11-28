package dev.okarin.rustygrade.ui

import android.content.Context
import android.content.Intent
import androidx.activity.result.contract.ActivityResultContracts

/**
 * The folder picker, asking for read *and* write access that survives restarts: tag editing needs
 * the write grant, and a read-only grant cannot be upgraded later without picking the folder again.
 */
class WritableTreePicker : ActivityResultContracts.OpenDocumentTree() {
    override fun createIntent(context: Context, input: android.net.Uri?): Intent =
        super.createIntent(context, input).addFlags(
            Intent.FLAG_GRANT_READ_URI_PERMISSION or
                Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION or
                Intent.FLAG_GRANT_PREFIX_URI_PERMISSION,
        )
}
