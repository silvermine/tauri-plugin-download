import { invoke } from '@tauri-apps/api/core'
import { listen, UnlistenFn } from '@tauri-apps/api/event';

/**
 * Creates a download operation.
 *
 * @param key - The key identifier.
 * @param url - The download URL  for the resource.
 * @param path - The download path on the filesystem.
 * @returns - The download operation.
 */
export async function create(key: string, url: string, path: string): Promise<Download> {
  return await DownloadImpl.create(await invoke<DownloadRecord>('plugin:download|create', { key, url, path }));
}

/**
 * Gets all download operations.
 *
 * @returns - The list of download operations.
 */
export async function list(): Promise<Download[]> {
  const records = await invoke<DownloadRecord[]>('plugin:download|list');
  return Promise.all(records.map((record) => DownloadImpl.create(record)));
}

/**
 * Gets a download operation.
 *
 * @param key - The key identifier.
 * @returns - The download operation.
 */
export async function get(key: string): Promise<Download> {
  return await DownloadImpl.create(await invoke<DownloadRecord>('plugin:download|get', { key }));
}

class DownloadImpl implements Download {
  private _unlisten: UnlistenFn[] = [];

  public constructor(record: DownloadRecord) {
    this.key = record.key;
    this.url = record.url;
    this.path = record.path;
    this.progress = record.progress;
    this.state = record.state;
  }
  
  key: string;
  url: string;
  path: string;
  progress: number;
  state: DownloadState;
  onState?: (state: DownloadState) => void;
  onProgress?: (progress: number) => void;  

  static async create(record: DownloadRecord): Promise<Download> {
    return new DownloadImpl(record).attach();
  }

  private async attach(): Promise<Download> {
    console.debug(`Attached listeners for ${this.key}`);

    // Listen for state events.
    this._unlisten.push(await listen<DownloadEvent>('tauri-plugin-download:state', (event) => {
      if (event.payload.key === this.key && this.onState) {
        this.onState(event.payload.state);
      }
    }));

    // Listen for progress events.
    this._unlisten.push(await listen<DownloadEvent>('tauri-plugin-download:progress', (event) => {
      if (event.payload.key === this.key && event.payload.progress && this.onProgress) {
        this.onProgress(event.payload.progress);
      }
    }));

    return this;
  }
  async detach(): Promise<void> {
    console.debug(`Detached listeners for ${this.key}`)

    // Unlisten from all events.
    for (let i = 0; i < this._unlisten.length; i++) {
      this._unlisten[i]();
    }
  }
  start(): Promise<Download> {
    return invoke('plugin:download|start', { key: this.key });
  }
  cancel(): Promise<Download> {
    return invoke('plugin:download|cancel', { key: this.key });
  }
  pause(): Promise<Download> {
    return invoke('plugin:download|pause', { key: this.key });
  }
  resume(): Promise<Download> {
    return invoke('plugin:download|resume', { key: this.key });
  }
}

/**
 * Represents a download operation.
 */
export interface Download extends DownloadRecord {
  /**
  * Callback when state is changed.
  */
  onState?: (state: DownloadState) => void;
  
  /**
  * Callback when progress is changed.
  */
  onProgress?: (progress: number) => void;

  /**
  * Detach event listeners.
  */
  detach(): Promise<void>;

  /**
  * Starts the download operation.
  */
  start(): Promise<Download>;

  /**
  * Cancels the download operation.
  */
  cancel(): Promise<Download>;

  /**
  * Pauses the download operation.
  */
  pause(): Promise<Download>;

  /**
  * Resumes the download operation.
  */
  resume(): Promise<Download>;
}

/**
 * Represents a download event.
 */
export interface DownloadEvent {
  key: string;
  state: DownloadState;
  progress?: number;
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
