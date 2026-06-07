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
         progress = derivedProgress(newTransferredBytes, newTotalBytes),
         transferredBytes = newTransferredBytes,
         totalBytes = newTotalBytes,
      )

   fun withStatus(newStatus: DownloadStatus): DownloadItem =
      copy(status = newStatus).let { updated ->
         val updatedTotalBytes = if (newStatus == DownloadStatus.Completed) {
            updated.totalBytes ?: updated.transferredBytes
         } else {
            updated.totalBytes
         }

         updated.copy(
            progress = derivedProgress(updated.transferredBytes, updatedTotalBytes, newStatus),
            totalBytes = updatedTotalBytes,
         )
      }

   private fun derivedProgress(
      transferredBytes: Long,
      totalBytes: Long?,
      currentStatus: DownloadStatus = status,
   ): Double =
      when {
         currentStatus == DownloadStatus.Completed -> 100.0
         totalBytes != null && totalBytes > 0 -> {
            (transferredBytes.toDouble() / totalBytes.toDouble()) * 100.0
         }
         else -> 0.0
      }
}
