package org.silvermine.downloadmanager

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * A value type that represents an item to be downloaded.
 * Used to track the status and progress of a download operation.
 */
@Serializable
data class DownloadItem(
   @SerialName("url")
   val url: String,

   @SerialName("path")
   val path: String,

   @SerialName("progress")
   val progress: Double = 0.0,

   @SerialName("transferredBytes")
   val transferredBytes: Long = 0,

   @SerialName("totalBytes")
   val totalBytes: Long? = null,

   @SerialName("status")
   val status: DownloadStatus = DownloadStatus.Idle,
) {
   fun withTransfer(newTransferredBytes: Long, newTotalBytes: Long?): DownloadItem =
      copy(
         progress = if (newTotalBytes != null && newTotalBytes > 0) {
            (newTransferredBytes.toDouble() / newTotalBytes.toDouble()) * 100.0
         } else {
            0.0
         },
         transferredBytes = newTransferredBytes,
         totalBytes = newTotalBytes,
      )

   fun withStatus(newStatus: DownloadStatus): DownloadItem =
      copy(
         progress = if (newStatus == DownloadStatus.Completed) 100.0 else progress,
         totalBytes = if (newStatus == DownloadStatus.Completed) totalBytes ?: transferredBytes else totalBytes,
         status = newStatus,
      )
}
