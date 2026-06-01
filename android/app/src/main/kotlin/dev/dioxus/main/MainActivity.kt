package dev.dioxus.main

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns

typealias BuildConfig = com.github.yeicor.colmap_openmvs_app.BuildConfig

class MainActivity : WryActivity() {

    companion object {
        const val REQUEST_PICK_FILE = 1001
        const val REQUEST_SAVE_FILE = 1002

        @JvmStatic
        external fun onFilesPickerResult(fds: IntArray, names: Array<String>)

        @JvmStatic
        external fun onSaveFileResult(fd: Int)
    }

    fun startFilePicker(mimeType: String, multiple: Boolean) {
        runOnUiThread {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = mimeType
                putExtra(Intent.EXTRA_ALLOW_MULTIPLE, multiple)
            }
            @Suppress("DEPRECATION")
            startActivityForResult(intent, REQUEST_PICK_FILE)
        }
    }

    fun startFileSaver(mimeType: String, suggestedName: String) {
        runOnUiThread {
            val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = mimeType
                putExtra(Intent.EXTRA_TITLE, suggestedName)
            }
            @Suppress("DEPRECATION")
            startActivityForResult(intent, REQUEST_SAVE_FILE)
        }
    }

    @Suppress("DEPRECATION", "OVERRIDE_DEPRECATION")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        when (requestCode) {
            REQUEST_PICK_FILE -> handlePickResult(resultCode, data)
            REQUEST_SAVE_FILE -> handleSaveResult(resultCode, data)
        }
    }

    private fun handlePickResult(resultCode: Int, data: Intent?) {
        if (resultCode != Activity.RESULT_OK || data == null) {
            onFilesPickerResult(IntArray(0), emptyArray())
            return
        }
        val uris = mutableListOf<Uri>()
        val clip = data.clipData
        if (clip != null) {
            for (i in 0 until clip.itemCount) uris += clip.getItemAt(i).uri
        } else {
            data.data?.let { uris += it }
        }
        val fds   = IntArray(uris.size) { -1 }
        val names = Array(uris.size) { "" }
        for ((i, uri) in uris.withIndex()) {
            names[i] = queryDisplayName(uri)
            val pfd = contentResolver.openFileDescriptor(uri, "r") ?: continue
            fds[i] = pfd.detachFd()
        }
        onFilesPickerResult(fds, names)
    }

    private fun handleSaveResult(resultCode: Int, data: Intent?) {
        if (resultCode != Activity.RESULT_OK || data?.data == null) {
            onSaveFileResult(-1)
            return
        }
        val pfd = contentResolver.openFileDescriptor(data.data!!, "w")
        onSaveFileResult(pfd?.detachFd() ?: -1)
    }

    private fun queryDisplayName(uri: Uri): String {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    if (idx >= 0) return cursor.getString(idx)
                }
            }
        return uri.lastPathSegment ?: "file"
    }
}
