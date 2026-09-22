package org.silvermine.downloadmanager

import kotlinx.serialization.Required
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * Persisted download record, stored in `downloads.json`. Mirrors the Rust
 * `DownloadRecord` in `crates/download-manager/src/models.rs`.
 *
 * Carries no `progress` — that is derived, and lives only on [DownloadItem],
 * the type sent to the frontend.
 */
@Serializable
internal data class DownloadRecord(
   @SerialName("url")
   val url: String,

   @SerialName("path")
   val path: String,

   @Required
   @SerialName("options")
   val options: CreateOptions = CreateOptions(),

   @Required
   @SerialName("receivedBytes")
   val receivedBytes: Long = 0L,

   /** `null` when the server did not supply a content length. */
   @SerialName("totalBytes")
   val totalBytes: Long? = null,

   @Required
   @SerialName("status")
   val status: DownloadStatus = DownloadStatus.Idle,
   val error: DownloadFailure? = null,

) {
   /**
    * Sets the byte counts. A `null` total means "this response did not report one",
    * not "there is none": a chunked resume cannot know a length an earlier response
    * already established, so a null never erases a total the record holds. Pass a
    * non-null total to widen or correct it.
    */
   fun withBytes(receivedBytes: Long, totalBytes: Long? = null): DownloadRecord =
      copy(receivedBytes = receivedBytes, totalBytes = totalBytes ?: this.totalBytes)

   fun withStatus(newStatus: DownloadStatus): DownloadRecord =
      copy(status = newStatus, error = if (newStatus == DownloadStatus.Failed) error else null)

   /** Final failures keep the partial and cannot overwrite a pause or cancellation. */
   fun failed(failure: DownloadFailure, partialLength: Long?): DownloadRecord? =
      if (status == DownloadStatus.InProgress) copy(
         status = DownloadStatus.Failed, error = failure, receivedBytes = partialLength ?: 0L,
      ) else null

   /**
    * Builds the public payload, computing `progress` from the byte counts.
    *
    * A completed download always reports 100%, even without a content length.
    * In flight without one it reports 0%, since [receivedBytes] is then the
    * only meaningful signal.
    */
   fun toItem(): DownloadItem {
      val progress = when {
         status == DownloadStatus.Completed -> 100.0
         totalBytes != null && totalBytes > 0L ->
            ((receivedBytes.toDouble() / totalBytes.toDouble()) * 100.0).coerceIn(0.0, 100.0)
         else -> 0.0
      }

      return DownloadItem(
         url = url,
         path = path,
         options = options,
         receivedBytes = receivedBytes,
         totalBytes = totalBytes,
         progress = progress,
         status = status,
         error = error,
      )
   }
}
