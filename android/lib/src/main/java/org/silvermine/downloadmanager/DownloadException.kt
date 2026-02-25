package org.silvermine.downloadmanager

/**
 * Exceptions thrown by download operations.
 */
sealed class DownloadException(message: String) : Exception(message) {
    /// Download item was not found for the given path.
    class NotFound(path: String) : DownloadException("Download not found: $path")
}
