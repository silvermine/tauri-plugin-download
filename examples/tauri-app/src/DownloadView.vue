<template>
   <div class="download-item">
      <div class="item-header">
         <h3 class="item-name">{{ downloadFilename }}</h3>
         <div class="item-actions" v-if="showActions">
            <button class="btn create-btn" type="button" @click="doCreate" v-if="canCreate">Create</button>
            <button class="btn start-btn" type="button" @click="doAction(DownloadAction.Start)" v-if="canStart">Start</button>
            <button class="btn cancel-btn" type="button" @click="doAction(DownloadAction.Cancel)" v-if="canCancel">Cancel</button>
            <button class="btn pause-btn" type="button" @click="doAction(DownloadAction.Pause)" v-if="canPause">Pause</button>
            <button class="btn resume-btn" type="button" @click="doAction(DownloadAction.Resume)" v-if="canResume">Resume</button>
         </div>
      </div>
      <p class="item-path">{{ currentDownload?.path }}</p>
      <div class="progress-bar">
         <div class="progress" :style="{ width: currentDownload.progress + '%' }" />
      </div>
      <div class="item-info">
         <p class="state-text">State: {{ currentDownload.status }}</p>
         <p class="progress-text">{{ progressLabel }}</p>
      </div>
      <div class="item-metrics">
         <p class="metric-text"><strong>progress:</strong> {{ progressLabel }}</p>
         <p class="metric-text" v-if="showByteMetrics"><strong>transferred_byte:</strong> {{ transferredBytesLabel }}</p>
         <p class="metric-text" v-if="showByteMetrics"><strong>total_byte:</strong> {{ totalBytesLabel }}</p>
      </div>
   </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue';
import {
   hasAction,
   hasAnyAction,
   DownloadAction,
   DownloadStatus,
   type DownloadWithAnyStatus,
   type Download,
   type UnexpectedStatusesForAction,
   type DownloadActionResponse,
} from 'tauri-plugin-download';
import { UnlistenFn } from '@tauri-apps/api/event';

const props = defineProps<{ download: DownloadWithAnyStatus, url?: string }>();

const currentDownload = ref<DownloadWithAnyStatus>(props.download);

const downloadFilename = computed(() => {
   return currentDownload.value.path.split('/').pop() ?? currentDownload.value.path;
});

const showActions = computed(() => { return hasAnyAction(currentDownload.value); });

const canCreate = computed(() => { return hasAction(currentDownload.value, DownloadAction.Create); });

const canStart = computed(() => { return hasAction(currentDownload.value, DownloadAction.Start); });

const canCancel = computed(() => { return hasAction(currentDownload.value, DownloadAction.Cancel); });

const canPause = computed(() => { return hasAction(currentDownload.value, DownloadAction.Pause); });

const canResume = computed(() => { return hasAction(currentDownload.value, DownloadAction.Resume); });

const showByteMetrics = computed(() => {
   return currentDownload.value.status !== DownloadStatus.Pending && currentDownload.value.status !== DownloadStatus.Idle;
});

const progressLabel = computed(() => {
   return currentDownload.value.progress.toFixed(2) + '%';
});

const transferredBytesLabel = computed(() => {
   return formatByteCount(currentDownload.value.transferredBytes);
});

const totalBytesLabel = computed(() => {
   return currentDownload.value.totalBytes === null ? 'unknown' : formatByteCount(currentDownload.value.totalBytes);
});


let unlisten: UnlistenFn | undefined;

onMounted(listenToEvents);
onUnmounted(() => { return unlisten?.(); });

function formatByteCount(bytes: number): string {
   const units = [ 'B', 'KiB', 'MiB', 'GiB', 'TiB' ];

   const formatter = new Intl.NumberFormat();

   let [ displayedBytes, unitIndex ] = [ bytes, 0 ];

   while (displayedBytes >= 1024 && unitIndex < (units.length - 1)) {
      displayedBytes /= 1024;
      unitIndex += 1;
   }

   if (unitIndex === 0) {
      return formatter.format(bytes) + ' ' + units[unitIndex];
   }

   return formatter.format(bytes) + ' bytes (' + displayedBytes.toFixed(2) + ' ' + units[unitIndex] + ')';
}

async function listenToEvents(): Promise<void> {
   if (unlisten || !hasAction(currentDownload.value, DownloadAction.Listen)) {
      return;
   }
   unlisten = await currentDownload.value.listen((updated) => {
      currentDownload.value = updated;
   });
}

function onError(error: Error): void {
   console.error(error);
}

type StatusHandlers<A extends DownloadAction> = Partial<{
   [S in UnexpectedStatusesForAction<A>]: (actualState: Download<S>) => void;
}>;

type ActionHandlers = Partial<{
   [K in DownloadAction]: StatusHandlers<K>;
}>;

const unexpectedStatusHandlers: ActionHandlers = {
   [DownloadAction.Start]: {
      [DownloadStatus.Canceled]: () => {
         // Tried to start the download but it was canceled instead
      },
   },
   [DownloadAction.Resume]: {
      [DownloadStatus.Canceled]: () => {
         // Tried to start the download but it was canceled instead
      },
   },

   [DownloadAction.Cancel]: {
      [DownloadStatus.Completed]: (): void => {
         // You'll probably want to delete the file since the user wanted to cancel
         // the download but wasn't able to before it completed
      },
      [DownloadStatus.InProgress]: (): void => {
         // There was a problem canceling the download
      },
   },

   [DownloadAction.Pause]: {
      [DownloadStatus.InProgress]: (): void => {
         // There was a problem pausing the download
      },
      [DownloadStatus.Completed]: (): void => {
         // The user tried to pause a completed download. This probably doesn't matter as
         // much as the other cases
      },
   },
};


function handleUnexpectedStatus(action: DownloadAction, result: DownloadActionResponse<DownloadAction>): void {
   const handlers = action in unexpectedStatusHandlers ? unexpectedStatusHandlers[action] : undefined;

   if (!handlers) {
      return;
   }

   const download = result.download;

   const status = download.status as keyof Required<typeof handlers>;

   if (download.status === status && handlers[status]) {
      handlers[status](download);
   }
}

async function doCreate(): Promise<void> {
   if (!hasAction(currentDownload.value, DownloadAction.Create) || !props.url) {
      return;
   }

   try {
      const result = await currentDownload.value.create(props.url);

      currentDownload.value = result.download;

      if (!result.isExpectedStatus) {
         handleUnexpectedStatus(DownloadAction.Create, result);
      }
   } catch(error) {
      onError(error instanceof Error ? error : new Error(String(error)));
   }
}

type NoArgAction = Exclude<DownloadAction, DownloadAction.Listen | DownloadAction.Create>;
async function doAction<A extends NoArgAction>(action: A): Promise<void> {
   if (!hasAction(currentDownload.value, action)) {
      return;
   }

   try {
      const result = await currentDownload.value[action]();

      currentDownload.value = result.download;

      if (!result.isExpectedStatus) {
         handleUnexpectedStatus(action, result);
      }
   } catch(error) {
      onError(error instanceof Error ? error : new Error(String(error)));
   }
}
</script>

<style scoped>
  .download-item {
    border: 1px solid #ddd;
    border-radius: 8px;
    padding: 15px;
    margin-bottom: 20px;
    box-shadow: 0 4px 6px rgba(0, 0, 0, 0.1);
  }

  .item-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 10px;
  }

  .item-name {
    font-size: 18px;
    font-weight: bold;
    margin: 0;
  }

  .item-actions button {
    margin-left: 5px;
  }

  .btn {
    padding: 6px;
    margin: 10px 5px;
    border: none;
    background-color: #007bff;
  }

  .item-info {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin: 0;
  }

  .item-metrics {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px;
    margin-top: 10px;
    text-align: left;
  }

  .progress-text {
    font-size: 14px;
    color: #555;
    margin: 0;
  }

  .metric-text {
    font-size: 12px;
    color: #555;
    margin: 0;
    word-break: break-word;
  }

  .progress-bar {
    background: #f0f0f0;
    border-radius: 4px;
    height: 10px;
    width: 100%;
    overflow: hidden;
    margin-bottom: 0;
  }

  .progress {
    background: #007bff;
    height: 100%;
    transition: width 0.3s;
  }

  .state-text {
    font-size: 14px;
    color: #555;
  }

  .item-path {
    font-size: 12px;
    color: #888;
    margin: 0 0 10px 0;
    text-align: left;
    word-break: break-all;
  }
  </style>
