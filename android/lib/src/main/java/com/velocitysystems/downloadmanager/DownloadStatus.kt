package com.velocitysystems.downloadmanager

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * Represents the various states of a download item.
 */
@Serializable
enum class DownloadStatus {
    /// Status could not be determined.
    @SerialName("unknown")
    Unknown,

    /// Download has not yet been created/persisted.
    @SerialName("pending")
    Pending,

    /// Download has been created and is ready to start.
    @SerialName("idle")
    Idle,

    /// Download is in progress.
    @SerialName("inProgress")
    InProgress,

    /// Download was in progress but has been paused.
    @SerialName("paused")
    Paused,

    /// Download was cancelled by the user.
    @SerialName("cancelled")
    Cancelled,

    /// Download completed.
    @SerialName("completed")
    Completed,
}
