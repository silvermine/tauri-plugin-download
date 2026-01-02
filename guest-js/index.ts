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
 * Gets a download by path.
 *
 * If the download exists in the store, returns it. If not found, returns a download in
 * {@link DownloadStatus.Pending} state (not persisted to store).
 *
 * A `Pending` download can have listeners attached and must be explicitly created via
 * `download.create(url)` to persist it to the store and transition to `Idle` state.
 *
 * @param path - The download path.
 * @returns The download operation.
 *
 * @example
 * ```ts
 * const download = await get('example/file.zip');
 * if (download.status === DownloadStatus.Pending) {
 *    await download.listen((d) => console.log(d.progress));
 *    const { download: created } = await download.create('https://example.com/file.zip');
 *    await created.start();
 * }
 * ```
 */
export async function get(path: string): Promise<DownloadWithAnyStatus> {
   const download = await invoke<DownloadState<DownloadStatus>>('plugin:download|get', { path });

   return attachDownload(download);
}

export * from './types';
