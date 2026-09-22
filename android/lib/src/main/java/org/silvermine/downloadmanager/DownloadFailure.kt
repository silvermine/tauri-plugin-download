package org.silvermine.downloadmanager

import android.system.ErrnoException
import android.system.OsConstants
import kotlinx.serialization.EncodeDefault
import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.Serializable
import java.io.IOException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.net.ConnectException
import java.security.cert.CertificateException
import javax.net.ssl.SSLException
import javax.net.ssl.SSLHandshakeException
import javax.net.ssl.SSLPeerUnverifiedException

/** Public diagnostic data, also persisted with failed downloads. */
@OptIn(ExperimentalSerializationApi::class)
@Serializable
data class DownloadFailure(
   val code: String,
   val message: String,
   val retryability: String,
   @EncodeDefault(EncodeDefault.Mode.NEVER)
   val httpStatus: Int? = null,
) {
   companion object {
      /** HTTP policy shared by request retries and final failure reporting. */
      fun http(status: Int): DownloadFailure = DownloadFailure(
         "http", "HTTP $status",
         if (status == 408 || status == 429 || status in 500..599 && status != 501 && status != 505)
            "transient" else "permanent",
         status,
      )

      /** Classifies network failures by native types, never diagnostic text. */
      fun network(error: Exception): DownloadFailure {
         val code: String
         val retryability: String
         when {
            error is SocketTimeoutException -> { code = "timeout"; retryability = "transient" }
            error is UnknownHostException || error is ConnectException -> { code = "connection"; retryability = "unknown" }
            error is SSLPeerUnverifiedException || error is SSLHandshakeException &&
               generateSequence<Throwable>(error) { it.cause }.any { it is CertificateException } -> {
               code = "tls"; retryability = "permanent"
            }
            error is SSLException -> { code = "tls"; retryability = "transient" }
            error is IOException -> { code = "connection"; retryability = "transient" }
            else -> { code = "unknown"; retryability = "unknown" }
         }
         return DownloadFailure(code, error.message ?: "Transfer failed", retryability)
      }

      /** Filesystem errors must not enter the network retry path. */
      fun file(error: Exception): DownloadFailure {
         val errno = generateSequence<Throwable>(error) { it.cause }
            .filterIsInstance<ErrnoException>().firstOrNull()?.errno
         val permanent = error is SecurityException || errno in listOf(
            OsConstants.EACCES, OsConstants.EPERM, OsConstants.ENOSPC, OsConstants.EROFS,
            OsConstants.ENOENT, OsConstants.ENOTDIR, OsConstants.EISDIR, OsConstants.EINVAL,
         )
         return DownloadFailure("file", error.message ?: "File operation failed", if (permanent) "permanent" else "unknown")
      }

      /** Keeps validation messages while adding the public command classification. */
      fun command(error: Exception): DownloadFailure {
         val code = commandErrorCode(error)
         if (code == "file") return file(error)
         return DownloadFailure(code, error.message ?: "Native command failed",
            if (code in listOf("invalid input", "invalid state", "download not found")) "permanent" else "unknown")
      }
   }
}

/** Carries a classified file failure through the worker's common exception handler. */
internal class TransferException(val failure: DownloadFailure) : Exception(failure.message)

/** Marks the filesystem boundary while preserving the original native cause. */
internal inline fun <T> fileOperation(action: () -> T): T = try {
   action()
} catch (error: Exception) {
   throw TransferException(DownloadFailure.file(error))
}
