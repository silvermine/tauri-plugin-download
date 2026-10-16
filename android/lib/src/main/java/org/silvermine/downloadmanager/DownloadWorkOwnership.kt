package org.silvermine.downloadmanager

import java.util.UUID

/**
 * Fences live commands against workers still unwinding asynchronous cancellation.
 * All access is under the manager monitor, together with store and file mutations.
 *
 * Restored paths allow persisted work to run after process startup; startup recovery
 * remains the manager's responsibility. Once a live command touches a path, only its
 * registered request can run. Finished and canceled paths need no retained entries.
 *
 * @param restoredPaths Paths loaded from the store before startup reconciliation.
 */
internal class DownloadWorkOwnership(restoredPaths: Collection<String> = emptyList()) {
   private val restored = restoredPaths.toMutableSet()
   private val requests = mutableMapOf<String, UUID>()

   /** Grants a replacement request exclusive ownership of [path]. */
   fun replace(path: String, request: UUID) {
      restored.remove(path)
      requests[path] = request
   }

   /** Prevents every existing request from modifying [path] after pause or cancel. */
   fun revoke(path: String) {
      restored.remove(path)
      requests.remove(path)
   }

   /** Retries keep their request ID; replacements and explicit stops invalidate it. */
   fun owns(path: String, request: UUID): Boolean =
      path in restored || requests[path] == request
}
