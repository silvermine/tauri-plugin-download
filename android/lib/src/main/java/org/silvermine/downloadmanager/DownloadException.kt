package org.silvermine.downloadmanager

/**
 * Exceptions thrown by download operations.
 */
sealed class DownloadException(message: String, cause: Throwable? = null) : Exception(message, cause) {
   /** Download item was not found for the given path. Matches the Rust and iOS message. */
   class NotFound(path: String) : DownloadException("Not Found: $path")

   /** Identifies persistence failures without inspecting an IOException's message. */
   class Store(cause: Exception) : DownloadException("Store Error: ${cause.message}", cause)
}

/** Stable command code carried through Tauri's native rejection bridge. */
fun commandErrorCode(error: Exception): String = when (error) {
   is DownloadException.NotFound -> "download not found"
   is DownloadException.Store -> "store"
   is IllegalArgumentException -> "invalid input"
   is IllegalStateException -> "invalid state"
   is java.io.IOException -> "file"
   else -> "unknown"
}
