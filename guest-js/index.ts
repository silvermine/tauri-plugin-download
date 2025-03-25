import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';

export class Download implements DownloadRecord {
   public key: string;
   public url: string;
   public path: string;
   public progress: number;
   public state: DownloadState;

   public constructor(record: DownloadRecord) {
      this.key = record.key;
      this.url = record.url;
      this.path = record.path;
      this.progress = record.progress;
      this.state = record.state;
   }

   /**
   * Starts the download.
   * @returns A promise with the updated download.
   */
   public async start(): Promise<Download> {
      return new Download(await invoke('plugin:download|start', { key: this.key }));
   }

   /**
   * Cancels the download.
   * @returns A promise with the updated download.
   */
   public async cancel(): Promise<Download> {
      return new Download(await invoke('plugin:download|cancel', { key: this.key }));
   }

   /**
   * Pauses the download.
   * @returns A promise with the updated download.
   */
   public async pause(): Promise<Download> {
      return new Download(await invoke('plugin:download|pause', { key: this.key }));
   }

   /**
   * Resumes the download.
   * @returns A promise with the updated download.
   */
   public async resume(): Promise<Download> {
      return new Download(await invoke('plugin:download|resume', { key: this.key }));
   }

   /**
   * Listen for changes to the download.
   * To avoid memory leaks, the `unlisten` function returned by the promise
   * should be called when no longer required.
   * @param onChanged - Callback function invoked when the download has changed.
   * @returns A promise with a function to remove the download listener.
   *
   * @example
   * ```ts
   * const unlisten = await download.listen((updatedDownload) => {
   *   console.log('Download:', updatedDownload);
   * });
   *
   * // To stop listening
   * unlisten();
   * ```
   */
   public async listen(listener: (download: Download) => void): Promise<UnlistenFn> {
      return listen<DownloadRecord>('tauri-plugin-download:changed', (event) => {
         if (event.payload.key === this.key) {
            listener(new Download(event.payload));
         }
      });
   }
}

/**
 * Represents a download record.
 */
export interface DownloadRecord {
  key: string;
  url: string;
  path: string;
  progress: number;
  state: DownloadState;
}

/**
* Represents the state of a download operation.
*/
export enum DownloadState {
  UNKNOWN = 'UNKNOWN',
  CREATED = 'CREATED',
  IN_PROGRESS = 'IN_PROGRESS',
  PAUSED = 'PAUSED',
  CANCELLED = 'CANCELLED',
  COMPLETED = 'COMPLETED'
}

/**
 * Creates a download operation.
 *
 * @param key - The key identifier.
 * @param url - The download URL for the resource.
 * @param path - The download path on the filesystem.
 * @returns - The download operation.
 */
export async function create(key: string, url: string, path: string): Promise<Download> {
   return new Download(await invoke<DownloadRecord>('plugin:download|create', { key, url, path }));
}

/**
 * Gets all download operations.
 *
 * @returns - The list of download operations.
 */
export async function list(): Promise<Download[]> {
   return (await invoke<DownloadRecord[]>('plugin:download|list'))
      .map((record) => { return new Download(record); });
}

/**
 * Gets a download operation.
 *
 * @param key - The key identifier.
 * @returns - The download operation.
 */
export async function get(key: string): Promise<Download> {
   return new Download(await invoke<DownloadRecord>('plugin:download|get', { key }));
}
