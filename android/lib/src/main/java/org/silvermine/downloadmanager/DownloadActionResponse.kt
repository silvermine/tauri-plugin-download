package org.silvermine.downloadmanager

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * Response from a download action containing the download item and status information.
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
