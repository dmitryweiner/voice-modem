interface ServiceWorkerHost {
  serviceWorker?: {
    controller: unknown;
    register: (url: string) => Promise<{ update: () => Promise<unknown> }>;
    addEventListener: (type: 'controllerchange', listener: () => void) => void;
  };
}

/**
 * Registers the offline worker once the page has loaded; failures only disable offline mode.
 * Phones resume a suspended page without a navigation, so the browser never looks for a new
 * worker by itself: we ask on every return to the foreground. When a new version takes control,
 * the page reloads to run it (the first worker of a fresh visit serves the same build, no reload).
 */
export function registerServiceWorker(
  nav: ServiceWorkerHost,
  win: Pick<Window, 'addEventListener'> & { location: Pick<Location, 'reload'> },
  doc: Pick<Document, 'addEventListener' | 'visibilityState'>,
): void {
  const sw = nav.serviceWorker;
  if (!sw) return;
  let controlled = Boolean(sw.controller);
  sw.addEventListener('controllerchange', () => {
    if (controlled) win.location.reload();
    controlled = true;
  });
  win.addEventListener('load', () => {
    sw.register('./sw.js')
      .then((registration) => {
        doc.addEventListener('visibilitychange', () => {
          if (doc.visibilityState === 'visible') registration.update().catch(() => {});
        });
      })
      .catch(() => {});
  });
}
