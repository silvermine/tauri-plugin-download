package com.velocitysystems.downloadmanager.example

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import com.velocitysystems.downloadmanager.DownloadItem
import com.velocitysystems.downloadmanager.DownloadStatus
import java.io.File

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DownloadsScreen(viewModel: DownloadsViewModel = viewModel()) {
    val state by viewModel.uiState.collectAsState()

    Scaffold(
        topBar = {
            TopAppBar(title = { Text("Downloads") })
        }
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(horizontal = 16.dp)
        ) {
            Text(
                text = "Enter a URL to download and click Get.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(top = 8.dp),
            )

            Spacer(modifier = Modifier.height(8.dp))

            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxWidth(),
            ) {
                OutlinedTextField(
                    value = state.downloadUrl,
                    onValueChange = viewModel::updateUrl,
                    placeholder = { Text("https://example.com/file.zip") },
                    singleLine = true,
                    modifier = Modifier.weight(1f),
                )

                Button(
                    onClick = viewModel::getDownload,
                    enabled = state.downloadUrl.isNotBlank(),
                ) {
                    Text("Get")
                }
            }

            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.padding(vertical = 8.dp),
            ) {
                Text("Auto-create")
                Switch(
                    checked = state.autoCreate,
                    onCheckedChange = viewModel::updateAutoCreate,
                )
            }

            LazyColumn(
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxSize(),
            ) {
                items(state.pendingDownloads, key = { it.path }) { pending ->
                    PendingDownloadRow(
                        pending = pending,
                        onCreate = { viewModel.createDownload(pending) },
                    )
                }

                items(state.downloads, key = { it.path }) { item ->
                    DownloadRow(
                        item = item,
                        onStart = { viewModel.startDownload(item.path) },
                        onPause = { viewModel.pauseDownload(item.path) },
                        onResume = { viewModel.resumeDownload(item.path) },
                        onCancel = { viewModel.cancelDownload(item.path) },
                    )
                }
            }
        }
    }
}

@Composable
private fun PendingDownloadRow(
    pending: PendingDownload,
    onCreate: () -> Unit,
) {
    Column(modifier = Modifier.fillMaxWidth()) {
        Text(
            text = File(pending.path).name,
            style = MaterialTheme.typography.titleSmall,
        )
        Text(
            text = "Status: pending",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(modifier = Modifier.height(4.dp))
        OutlinedButton(onClick = onCreate) {
            Text("Create")
        }
    }
}

@Composable
private fun DownloadRow(
    item: DownloadItem,
    onStart: () -> Unit,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onCancel: () -> Unit,
) {
    Column(modifier = Modifier.fillMaxWidth()) {
        Text(
            text = File(item.path).name,
            style = MaterialTheme.typography.titleSmall,
        )

        LinearProgressIndicator(
            progress = { (item.progress / 100.0).toFloat() },
            modifier = Modifier
                .fillMaxWidth()
                .padding(vertical = 4.dp),
        )

        Text(
            text = "Status: ${item.status.name} — ${String.format("%.0f", item.progress)}%",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        Spacer(modifier = Modifier.height(4.dp))

        when (item.status) {
            DownloadStatus.Idle -> {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = onStart) { Text("Start") }
                    OutlinedButton(
                        onClick = onCancel,
                        colors = ButtonDefaults.outlinedButtonColors(
                            contentColor = MaterialTheme.colorScheme.error,
                        ),
                    ) { Text("Cancel") }
                }
            }
            DownloadStatus.InProgress -> {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = onPause) { Text("Pause") }
                    OutlinedButton(
                        onClick = onCancel,
                        colors = ButtonDefaults.outlinedButtonColors(
                            contentColor = MaterialTheme.colorScheme.error,
                        ),
                    ) { Text("Cancel") }
                }
            }
            DownloadStatus.Paused -> {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = onResume) { Text("Resume") }
                    OutlinedButton(
                        onClick = onCancel,
                        colors = ButtonDefaults.outlinedButtonColors(
                            contentColor = MaterialTheme.colorScheme.error,
                        ),
                    ) { Text("Cancel") }
                }
            }
            else -> { /* No actions for other states */ }
        }
    }
}
