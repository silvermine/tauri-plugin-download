package com.velocitysystems.downloadmanager

import java.net.URI

/**
 * Parses and validates a download path string.
 * Checks that the path is not empty, is an absolute path and contains a filename.
 */
fun parsePath(pathString: String): String {
    if (pathString.isEmpty()) {
        throw IllegalArgumentException("Path cannot be empty")
    }

    val path: String
    if (pathString.startsWith("file://")) {
        val uri = try {
            URI(pathString)
        } catch (e: Exception) {
            throw IllegalArgumentException("Invalid file URL: $pathString")
        }
        if (uri.scheme != "file") {
            throw IllegalArgumentException("Invalid file URL: $pathString")
        }
        path = uri.path ?: throw IllegalArgumentException("Invalid file URL: $pathString")
    } else if (pathString.startsWith("/")) {
        path = pathString
    } else {
        throw IllegalArgumentException("Path must be absolute")
    }

    val fileName = path.substringAfterLast("/")
    if (fileName.isEmpty()) {
        throw IllegalArgumentException("Path must have a filename")
    }

    return path
}

/**
 * Parses and validates a download URL string.
 * Checks that the URL is valid, has a valid scheme (http or https) and has a valid host.
 */
fun parseUrl(urlString: String): String {
    val uri = try {
        URI(urlString)
    } catch (e: Exception) {
        throw IllegalArgumentException("Invalid URL: $urlString")
    }

    val scheme = uri.scheme?.lowercase()
    if (scheme != "http" && scheme != "https") {
        throw IllegalArgumentException("Invalid URL scheme '${scheme ?: "none"}': must be http or https")
    }

    val host = uri.host
    if (host.isNullOrEmpty()) {
        throw IllegalArgumentException("URL must have a host")
    }

    return urlString
}
