package com.velocitysystems.downloadmanager

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

    @SerialName("status")
    val status: DownloadStatus = DownloadStatus.Idle,
) {
    fun withProgress(newProgress: Double): DownloadItem =
        copy(progress = newProgress, status = DownloadStatus.InProgress)

    fun withStatus(newStatus: DownloadStatus): DownloadItem =
        copy(
            progress = if (newStatus == DownloadStatus.Completed) 100.0 else progress,
            status = newStatus,
        )
}
