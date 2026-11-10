import './style.css';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { startAudio } from './audio/engine';
import { registerServiceWorker } from './pwa';
import { browserStorage } from './storage';
import { coreClient } from './worker/client';

const root = document.querySelector('#root');
if (!root) throw new Error('index.html has no #root');

const core = coreClient(() => new Worker(new URL('./worker/modem.worker.ts', import.meta.url), { type: 'module' }));

createRoot(root).render(
  <StrictMode>
    <App storage={browserStorage()} languages={navigator.languages} core={core} startAudio={startAudio} />
  </StrictMode>,
);
if (import.meta.env.PROD) registerServiceWorker(navigator, window, document);
