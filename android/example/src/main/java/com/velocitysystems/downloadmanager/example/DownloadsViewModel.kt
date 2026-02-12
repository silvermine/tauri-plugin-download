package com.velocitysystems.downloadmanager.example

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.velocitysystems.downloadmanager.DownloadItem
import com.velocitysystems.downloadmanager.DownloadManager
import com.velocitysystems.downloadmanager.DownloadStatus
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.io.File

data class PendingDownload(
    val url: String,
    val path: String,
)

data class DownloadsUiState(
    val downloads: List<DownloadItem> = emptyList(),
    val pendingDownloads: List<PendingDownload> = emptyList(),
    val downloadUrl: String = "",
    val autoCreate: Boolean = true,
)

class DownloadsViewModel(application: Application) : AndroidViewModel(application) {
    private val manager = DownloadManager.getInstance(application)

    private val _uiState = MutableStateFlow(DownloadsUiState())
    val uiState: StateFlow<DownloadsUiState> = _uiState.asStateFlow()

    init {
        _uiState.value = _uiState.value.copy(downloads = manager.list())

        viewModelScope.launch {
            manager.changed.collect { item ->
                android.util.Log.d("DownloadsViewModel", "[${File(item.path).name}] ${item.status} - ${String.format("%.0f", item.progress)}%")
                _uiState.value = _uiState.value.copy(downloads = manager.list())
            }
        }
    }

    fun updateUrl(url: String) {
        _uiState.value = _uiState.value.copy(downloadUrl = url)
    }

    fun updateAutoCreate(autoCreate: Boolean) {
        _uiState.value = _uiState.value.copy(autoCreate = autoCreate)
    }

    fun getDownload() {
        val state = _uiState.value
        val url = state.downloadUrl.trim()
        if (url.isEmpty()) return

        try {
            val uri = java.net.URI(url)
            if (uri.scheme == null || uri.host == null) return
        } catch (_: Exception) {
            return
        }

        val filename = url.substringAfterLast("/")
        val downloadsDir = getApplication<Application>().getExternalFilesDir(null)
        val path = File(downloadsDir, filename).absolutePath

        val download = manager.get(path)

        if (download.status == DownloadStatus.Pending) {
            if (state.autoCreate) {
                manager.create(path, url)
                _uiState.value = _uiState.value.copy(
                    downloadUrl = "",
                    downloads = manager.list(),
                )
            } else {
                _uiState.value = _uiState.value.copy(
                    downloadUrl = "",
                    pendingDownloads = state.pendingDownloads + PendingDownload(url = url, path = path),
                )
            }
        } else {
            _uiState.value = _uiState.value.copy(downloadUrl = "")
        }
    }

    fun createDownload(pending: PendingDownload) {
        manager.create(pending.path, pending.url)
        _uiState.value = _uiState.value.copy(
            pendingDownloads = _uiState.value.pendingDownloads.filter { it.path != pending.path },
            downloads = manager.list(),
        )
    }

    fun startDownload(path: String) {
        manager.start(path)
    }

    fun pauseDownload(path: String) {
        manager.pause(path)
    }

    fun resumeDownload(path: String) {
        manager.resume(path)
    }

    fun cancelDownload(path: String) {
        manager.cancel(path)
    }
}
