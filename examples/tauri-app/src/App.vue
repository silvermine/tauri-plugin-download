<template>
   <main class="container">
      <h1>tauri-plugin-download</h1>

      <div class="row">
         <a href="https://vitejs.dev" target="_blank">
            <img src="/vite.svg" class="logo vite" alt="Vite logo">
         </a>
         <a href="https://tauri.app" target="_blank">
            <img src="/tauri.svg" class="logo tauri" alt="Tauri logo">
         </a>
         <a href="https://vuejs.org/" target="_blank">
            <img src="./assets/vue.svg" class="logo vue" alt="Vue logo">
         </a>
      </div>
      <p>Enter a URL to download and click <em>Get.</em></p>
      <!-- Create Download -->
      <form class="row" @submit.prevent>
         <input id="url-input" v-model="downloadURL" placeholder="https://foo.com/sample.zip">
         <button type="button" @click="getDownload">Get</button>
         <label class="toggle-label">
            <input type="checkbox" v-model="autoCreate">
            Auto-create
         </label>
      </form>
      <!-- Manage Downloads -->
      <div class="download-list">
         <DownloadView v-for="item in downloads" :key="item.download.key" v-bind="item" />
      </div>
   </main>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { appDataDir, join } from '@tauri-apps/api/path';
import { get, list, DownloadStatus, DownloadWithAnyStatus } from 'tauri-plugin-download';
import DownloadView from './DownloadView.vue';

interface DownloadViewItem {
   download: DownloadWithAnyStatus;
   url?: string;
   path?: string;
}

const downloadURL = ref(''),
      downloads = ref<DownloadViewItem[]>(),
      autoCreate = ref(true);

onMounted(async () => {
   const items = await list();

   downloads.value = items.map((download) => { return { download }; });
});

async function getDownload() {
   const key = getFilenameFromURL(downloadURL.value);

   if (!key) {
      console.error('Could not get filename from URL', downloadURL.value);
      return;
   }

   const path = await join(await appDataDir(), 'downloads', key);

   let download = await get(key);

   if (download.status === DownloadStatus.Pending && autoCreate.value) {
      const { download: created } = await download.create(downloadURL.value, path);

      download = created;
   }

   downloads.value?.push({ download, url: downloadURL.value, path });
   downloadURL.value = '';
}

function getFilenameFromURL(url: string): string | null {
   try {
      const urlObject = new URL(url),
            pathname = urlObject.pathname,
            filename = pathname.substring(pathname.lastIndexOf('/') + 1);

      return filename;
   } catch(error) {
      // Handle cases where the URL is invalid
      console.error('Invalid URL:', error);
      return null;
   }
}
</script>

<style scoped>
.download-list {
  padding: 20px;
}
</style>

<style scoped>
.logo.vite:hover {
  filter: drop-shadow(0 0 2em #747bff);
}

.logo.vue:hover {
  filter: drop-shadow(0 0 2em #249b73);
}
</style>
<style>
:root {
  font-family: Inter, Avenir, Helvetica, Arial, sans-serif;
  font-size: 16px;
  line-height: 24px;
  font-weight: 400;

  color: #0f0f0f;
  background-color: #f6f6f6;

  font-synthesis: none;
  text-rendering: optimizeLegibility;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  -webkit-text-size-adjust: 100%;
}

.container {
  margin: 0;
  padding-top: 10vh;
  display: flex;
  flex-direction: column;
  justify-content: center;
  text-align: center;
}

.logo {
  height: 6em;
  padding: 1.5em;
  will-change: filter;
  transition: 0.75s;
}

.logo.tauri:hover {
  filter: drop-shadow(0 0 2em #24c8db);
}

.row {
  display: flex;
  justify-content: center;
}

a {
  font-weight: 500;
  color: #646cff;
  text-decoration: inherit;
}

a:hover {
  color: #535bf2;
}

h1 {
  text-align: center;
}

input,
button {
  border-radius: 8px;
  border: 1px solid transparent;
  padding: 0.6em 1.2em;
  font-size: 1em;
  font-weight: 500;
  font-family: inherit;
  color: #0f0f0f;
  background-color: #ffffff;
  transition: border-color 0.25s;
  box-shadow: 0 2px 2px rgba(0, 0, 0, 0.2);
}

button {
  cursor: pointer;
}

button:hover {
  border-color: #396cd8;
}
button:active {
  border-color: #396cd8;
  background-color: #e8e8e8;
}

input,
button {
  outline: none;
}

#url-input {
  margin-right: 5px;
}

.toggle-label {
  display: flex;
  align-items: center;
  margin-left: 10px;
  gap: 5px;
  cursor: pointer;
}

@media (prefers-color-scheme: dark) {
  :root {
    color: #f6f6f6;
    background-color: #2f2f2f;
  }

  a:hover {
    color: #24c8db;
  }

  input,
  button {
    color: #ffffff;
    background-color: #0f0f0f98;
  }
  button:active {
    background-color: #0f0f0f69;
  }
}
</style>
