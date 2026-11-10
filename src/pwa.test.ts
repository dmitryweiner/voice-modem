import { describe, expect, it, vi } from 'vitest';
import { registerServiceWorker } from './pwa';

type Handler = () => void;

/** Event target that remembers listeners by type and fires them on demand. */
function fakeTarget() {
  const listeners = new Map<string, Handler[]>();
  return {
    addEventListener: vi.fn<(type: string, fn: Handler) => void>((type, fn) => {
      listeners.set(type, [...(listeners.get(type) ?? []), fn]);
    }),
    fire: (type: string) => listeners.get(type)?.forEach((fn) => fn()),
  };
}

function fakeDocument(): ReturnType<typeof fakeTarget> & { visibilityState: DocumentVisibilityState } {
  return { ...fakeTarget(), visibilityState: 'visible' };
}

function fakeWindow() {
  return { ...fakeTarget(), location: { reload: vi.fn<() => void>() } };
}

interface Registration {
  update: () => Promise<unknown>;
}

function setup({ controlled = true, register }: { controlled?: boolean; register?: () => Promise<Registration> } = {}) {
  const update = vi.fn<() => Promise<unknown>>(() => Promise.resolve());
  const container = {
    ...fakeTarget(),
    controller: controlled ? {} : null,
    register: vi.fn<(url: string) => Promise<Registration>>(register ?? (() => Promise.resolve({ update }))),
  };
  const win = fakeWindow();
  const doc = fakeDocument();
  registerServiceWorker({ serviceWorker: container }, win, doc);
  const loaded = async () => {
    win.fire('load');
    await Promise.resolve();
  };
  return { container, win, doc, update, loaded };
}

describe('registerServiceWorker', () => {
  it('registers the worker next to the page after load', async () => {
    const { container, loaded } = setup();
    expect(container.register).not.toHaveBeenCalled();
    await loaded();
    expect(container.register).toHaveBeenCalledWith('./sw.js');
  });

  it('does nothing when service workers are not supported', () => {
    const win = fakeWindow();
    const doc = fakeDocument();
    registerServiceWorker({}, win, doc);
    expect(win.addEventListener).not.toHaveBeenCalled();
    expect(doc.addEventListener).not.toHaveBeenCalled();
  });

  it('swallows registration errors', async () => {
    const { loaded, doc } = setup({ register: () => Promise.reject(new Error('nope')) });
    await expect(loaded()).resolves.toBeUndefined();
    expect(() => doc.fire('visibilitychange')).not.toThrow();
  });

  it('checks for a new version whenever the page comes back to the foreground', async () => {
    const { doc, update, loaded } = setup();
    await loaded();
    doc.visibilityState = 'hidden';
    doc.fire('visibilitychange');
    expect(update).not.toHaveBeenCalled();
    doc.visibilityState = 'visible';
    doc.fire('visibilitychange');
    expect(update).toHaveBeenCalledTimes(1);
  });

  it('swallows update errors, e.g. when offline', async () => {
    const { doc, update, loaded } = setup();
    update.mockImplementation(() => Promise.reject(new Error('offline')));
    await loaded();
    expect(() => doc.fire('visibilitychange')).not.toThrow();
  });

  it('reloads the page once a new version takes control', () => {
    const { container, win } = setup();
    container.fire('controllerchange');
    expect(win.location.reload).toHaveBeenCalledTimes(1);
  });

  it('does not reload when the very first worker takes control of the page', () => {
    const { container, win } = setup({ controlled: false });
    container.fire('controllerchange');
    expect(win.location.reload).not.toHaveBeenCalled();
    container.fire('controllerchange');
    expect(win.location.reload).toHaveBeenCalledTimes(1);
  });
});
