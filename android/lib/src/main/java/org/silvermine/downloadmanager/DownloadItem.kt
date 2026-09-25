package org.silvermine.downloadmanager

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * Emit-only payload sent to the frontend. Built from a [DownloadRecord] with
 * `progress` derived from [receivedBytes] / [totalBytes]; never persisted, so
 * nothing internal to the download machinery leaks into the frontend contract.
 * [totalBytes] is `null` when the server supplied no content length, and
 * serializes as an explicit JSON `null`.
 *
 * No property carries a default, so every key here is written whatever
 * `encodeDefaults` the caller's `Json` uses, rather than depending on the
 * TypeScript layer to coalesce it (attachDownload in `guest-js/actions.ts`).
 * [CreateOptions] carries one but marks it `@Required`, so it is written too.
 */
@Serializable
data class DownloadItem(
   @SerialName("url")
   val url: String,

   @SerialName("path")
   val path: String,

   /** Network policy fixed when this download was created. */
   @SerialName("options")
   val options: CreateOptions,

   @SerialName("receivedBytes")
   val receivedBytes: Long,

   @SerialName("totalBytes")
   val totalBytes: Long?,

   @SerialName("progress")
   val progress: Double,

   @SerialName("status")
   val status: DownloadStatus,
   val error: DownloadFailure? = null,

)
