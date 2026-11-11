import { mount } from 'svelte';
import { getCurrentWindow } from '@tauri-apps/api/window';
import DownloadLogsWindow from './lib/components/DownloadLogsWindow.svelte';
import DownloadAuthWindow from './lib/components/DownloadAuthWindow.svelte';
import App from './App.svelte';
import './styles.css';

const label = getCurrentWindow().label;
const Root = label === 'download-auth' ? DownloadAuthWindow : label === 'download-logs' ? DownloadLogsWindow : App;
mount(Root, { target: document.getElementById('app')! });
