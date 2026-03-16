package org.silvermine.downloadmanager

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * Response from a download action containing the download item and its expected status.
 *
 * When the download is already in the expected state for the requested action, this is
 * returned as a soft rejection rather than throwing an exception. The caller can check
 * [isExpectedStatus] to determine if the action had the intended effect:
 * - `true`: The action succeeded and the download transitioned to [expectedStatus].
 * - `false`: The download was already in a different state (e.g., calling `start` on an
 *   already-paused download). The [download] reflects the current state, and
 *   [expectedStatus] shows what the action would have produced.
 */
@Serializable
data class DownloadActionResponse(
   @SerialName("download")
   val download: DownloadItem,

   @SerialName("expectedStatus")
   val expectedStatus: DownloadStatus,

   @SerialName("isExpectedStatus")
   val isExpectedStatus: Boolean,
) {
   companion object {
      fun new(download: DownloadItem): DownloadActionResponse =
         DownloadActionResponse(
            download = download,
            expectedStatus = download.status,
            isExpectedStatus = true,
         )

      fun withExpectedStatus(download: DownloadItem, expectedStatus: DownloadStatus): DownloadActionResponse =
         DownloadActionResponse(
            download = download,
            expectedStatus = expectedStatus,
            isExpectedStatus = download.status == expectedStatus,
         )
   }
}
