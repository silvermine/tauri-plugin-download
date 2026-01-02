import type { UnlistenFn } from '@tauri-apps/api/event';


/**
 * Represents the status of a download operation.
 *
 * Use the `status` field on a {@link Download} object to determine which actions
 * are available. TypeScript will automatically narrow the available methods based
 * on the status.
 *
 * @example
 * ```ts
 * if (download.status === DownloadStatus.Idle) {
 *    await download.start(); // TypeScript knows start() is available
 * }
 * ```
 */
export enum DownloadStatus {

   /** Download exists in memory but has not been persisted to the store. */
   Pending = 'pending',

   /** Download has been persisted to the store but has not started downloading. */
   Idle = 'idle',

   /** Download is in progress. */
   InProgress = 'inProgress',

   /** Download was paused by the user. */
   Paused = 'paused',

   /** Download was cancelled by the user. */
   Cancelled = 'cancelled',

   /** Download has completed successfully. */
   Completed = 'completed',

   /** Download status is unknown. */
   Unknown = 'unknown',
}

export enum DownloadAction {
   Listen = 'listen',
   Create = 'create',
   Start = 'start',
   Resume = 'resume',
   Pause = 'pause',
   Cancel = 'cancel',
}

export interface DownloadState<S extends DownloadStatus> {
   url: string;
   path: string;
   progress: number;
   status: S;
}

export interface DownloadActionResponse<A extends DownloadAction = DownloadAction> {
   download: DownloadWithAnyStatus;
   expectedStatus: ExpectedStatusesForAction<A>;
   isExpectedStatus: boolean;
   error?: Error;
}

export interface AllDownloadActions {

   /**
    * Listen for changes to the download state. To avoid memory leaks, the `unlisten`
    * function returned by the promise should be called when no longer required.
    *
    * @param onChanged Callback function invoked when the download has changed.
    * @returns A promise with a function to remove the download listener.
    *
    * @example
    * ```ts
    * const unlisten = await download.listen((updatedDownload) => {
    *   console.log('Download:', updatedDownload);
    *   if (updatedDownload.status === DownloadStatus.Paused) {
    *     updatedDownload.resume(); // TypeScript knows this is valid
    *   }
    * });
    *
    * // To stop listening
    * unlisten();
    * ```
    */
   [DownloadAction.Listen]: (listener: (download: DownloadWithAnyStatus) => void) => Promise<UnlistenFn>;
   [DownloadAction.Create]: (url: string) => Promise<DownloadActionResponse<DownloadAction.Create>>;
   [DownloadAction.Start]: () => Promise<DownloadActionResponse<DownloadAction.Start>>;
   [DownloadAction.Resume]: () => Promise<DownloadActionResponse<DownloadAction.Resume>>;
   [DownloadAction.Pause]: () => Promise<DownloadActionResponse<DownloadAction.Pause>>;
   [DownloadAction.Cancel]: () => Promise<DownloadActionResponse<DownloadAction.Cancel>>;
}

// Only these actions are allowed for each given DownloadStatus:
export const allowedActions = {
   [DownloadStatus.Pending]: [
      DownloadAction.Listen,
      DownloadAction.Create,
   ],
   [DownloadStatus.Idle]: [
      DownloadAction.Listen,
      DownloadAction.Start,
      DownloadAction.Cancel,
   ],
   [DownloadStatus.InProgress]: [
      DownloadAction.Listen,
      DownloadAction.Pause,
      DownloadAction.Cancel,
   ],
   [DownloadStatus.Paused]: [
      DownloadAction.Listen,
      DownloadAction.Resume,
      DownloadAction.Cancel,
   ],
   [DownloadStatus.Completed]: [],
   [DownloadStatus.Cancelled]: [],
   [DownloadStatus.Unknown]: [
      DownloadAction.Listen,
   ],
} as const satisfies Record<DownloadStatus, DownloadAction[] | []>;

export const expectedStatusesForAction = {
   [DownloadAction.Create]: [ DownloadStatus.Idle ],
   [DownloadAction.Start]: [ DownloadStatus.InProgress ],
   [DownloadAction.Resume]: [ DownloadStatus.InProgress ],
   [DownloadAction.Pause]: [ DownloadStatus.Paused ],
   [DownloadAction.Cancel]: [ DownloadStatus.Cancelled ],

   // Everything but "unknown" is valid:
   [DownloadAction.Listen]: [
      DownloadStatus.Pending,
      DownloadStatus.Idle,
      DownloadStatus.InProgress,
      DownloadStatus.Paused,
      DownloadStatus.Cancelled,
      DownloadStatus.Completed,
   ],
} as const satisfies Record<DownloadAction, DownloadStatus[] | []>;

type ActionsFns<S extends DownloadStatus> = Pick<AllDownloadActions, typeof allowedActions[S][number]>;
type AllowedActionsForStatus<S extends DownloadStatus> = ActionsFns<S> extends never ? object : ActionsFns<S>;

export type Download<S extends DownloadStatus> = DownloadState<S> & AllowedActionsForStatus<S>;

/**
 * Union type representing a download in any status.
 *
 * To narrow the type to a more specific Download status, use either
 * {@link hasAction `hasAction`} or the `status` field as a discriminator.
 *
 * @example
 * ```ts
 * if (hasAction(download, DownloadAction.Start)) {
 *    await download.start();
 * }
 *
 * // Or:
 * if (download.status === DownloadStatus.Idle) {
 *   await download.start(); // TypeScript knows start() is available
 * }
 * ```
 */
export type DownloadWithAnyStatus = { [T in DownloadStatus]: Download<T> }[DownloadStatus];

export type ExpectedStatusesForAction<A extends DownloadAction> = (typeof expectedStatusesForAction)[A][number];
export type UnexpectedStatusesForAction<A extends DownloadAction> = Exclude<DownloadStatus, ExpectedStatusesForAction<A>>;

export type ExpectedStatesForAction<A extends DownloadAction> = Extract<DownloadWithAnyStatus, Pick<AllDownloadActions, A>>;
export type UnexpectedStatesForAction<A extends DownloadAction> = Exclude<DownloadWithAnyStatus, ExpectedStatesForAction<A>>;

export function hasAction<A extends DownloadAction>(download: DownloadWithAnyStatus, actionName: A): download is Extract<DownloadWithAnyStatus, Pick<AllDownloadActions, A>> {
   return (allowedActions[download.status] as DownloadAction[]).includes(actionName);
}

/**
 * @returns `true` if the download has actions available, i.e. not in a terminal state.
 */
export function hasAnyAction(download: DownloadWithAnyStatus): download is Exclude<DownloadWithAnyStatus, Download<DownloadStatus.Completed> | Download<DownloadStatus.Cancelled>> {
   return download.status !== DownloadStatus.Completed && download.status !== DownloadStatus.Cancelled;
}
