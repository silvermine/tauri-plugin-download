import { invoke } from '@tauri-apps/api/core';
import { DownloadState, DownloadStatus, DownloadWithAnyStatus } from './types';
import { attachDownload } from './actions';
export { attachDownload };

/**
 * Lists all persisted download operations.
 *
 * @returns All downloads that have been created and persisted to the store.
 *
 * @example
 * ```ts
 * const downloads = await list();
 * for (const download of downloads) {
 *    if (hasAction(download, DownloadAction.Resume)) {
 *       await download.resume();
 *    }
 * }
 * ```
 */
export async function list(): Promise<DownloadWithAnyStatus[]> {
   return (await invoke<DownloadState<DownloadStatus>[]>('plugin:download|list'))
      .map((item) => { return attachDownload(item); });
}

/**
 * Gets a download by key.
 *
 * If the download exists in the store, returns it. If not found, returns a download in
 * {@link DownloadStatus.Pending} state (not persisted to store).
 *
 * A `Pending` download can have listeners attached and must be explicitly created via
 * `download.create(url, path)` to persist it to the store and transition to `Idle` state.
 *
 * @param key - Unique identifier for the download.
 * @returns The download operation.
 *
 * @example
 * ```ts
 * const download = await get('my-download');
 * if (download.status === DownloadStatus.Pending) {
 *    await download.listen((d) => console.log(d.progress));
 *    const { download: created } = await download.create(url, path);
 *    await created.start();
 * }
 * ```
 */
export async function get(key: string): Promise<DownloadWithAnyStatus> {
   const download = await invoke<DownloadState<DownloadStatus>>('plugin:download|get', { key });

   return attachDownload(download);
}

export * from './types';
