import { useEffect, useRef, useState, type ChangeEvent, type FormEvent } from 'react';
import { audioErrorKind, type AudioErrorKind, type AudioSession, type StartAudio } from './audio/engine';
import { CLIP_LEVEL, SILENT, type Meter } from './audio/meter';
import type { Messages } from './i18n';
import { MAX_NAME_LENGTH } from './storage';
import type { CoreClient } from './worker/client';
import { MODE_BIT_RATES, type StationOptions } from './worker/protocol';

export interface ModemPanelProps {
  t: Messages;
  core: CoreClient;
  startAudio: StartAudio;
  /** Makes an address a received file can be saved from. */
  fileUrl: (bytes: Uint8Array) => string;
  /** False until the core has loaded: the microphone cannot be turned on. */
  coreReady: boolean;
  /** The name of the station and whether it is a push-to-talk radio; they take effect when the microphone is turned on. */
  station: StationOptions;
  onStationChange: (station: StationOptions) => void;
}

type Link =
  | { status: 'off' }
  | { status: 'starting' }
  | { status: 'on'; session: AudioSession; maxTextBytes: number; manualPtt: boolean; oneWay: boolean }
  | { status: 'error'; kind: AudioErrorKind };

/**
 * A file: offered and waiting for consent, moving, whole on the other side,
 * or stopped — nobody answers (received: the content is damaged), the other
 * side refused or gave up, this side gave up.
 */
type FileStatus = 'offered' | 'moving' | 'done' | 'failed' | 'refused' | 'cancelled';

type Item =
  | { key: number; kind: 'received'; text: string; snrDb: number }
  /**
   * `id` is the modem's, and `run` tells which modem: each one started counts
   * from one again. `attempt` grows with every repeat.
   */
  | {
      key: number;
      kind: 'sent';
      run: number;
      id: number;
      text: string;
      status: 'sending' | 'sent' | 'delivered' | 'failed' | 'cancelled';
      attempt: number;
    }
  | {
      key: number;
      kind: 'file';
      run: number;
      id: number;
      outgoing: boolean;
      name: string;
      size: number;
      /** Bytes that are through. */
      done: number;
      secondsLeft: number | null;
      status: FileStatus;
      /** Sent: the content, to offer it again; what it takes at the speed it was offered at, seconds. */
      bytes?: Uint8Array;
      seconds?: number;
      /** Broadcast: the symbols that went, and how many make the file (0 until the first went). */
      cast?: { symbols: number; needed: number };
      /** Received: where to save it from. */
      url?: string;
    }
  | { key: number; kind: 'peer'; name: string }
  | { key: number; kind: 'lost' };

/** A file that takes longer than this is offered with a warning, seconds. */
const SLOW_FILE = 600;
/** How often the level and the spectrum are redrawn, ms. */
const METER_INTERVAL = 50;

const encoder = new TextEncoder();

function errorText(t: Messages, kind: AudioErrorKind): string {
  return { denied: t.micDenied, noDevice: t.micNoDevice, unsupported: t.micUnsupported, failed: t.micFailed }[kind];
}

function InputMeter({ t, session }: { t: Messages; session: AudioSession }) {
  const [meter, setMeter] = useState<Meter>(SILENT);
  useEffect(() => {
    const timer = setInterval(() => setMeter(session.meter()), METER_INTERVAL);
    return () => clearInterval(timer);
  }, [session]);

  const percent = Math.round(meter.peak * 100);
  const clipping = meter.peak >= CLIP_LEVEL;
  return (
    <div className="meter">
      <div
        id="level"
        className={clipping ? 'level clipping' : 'level'}
        role="meter"
        aria-label={t.inputLevel}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <div style={{ inlineSize: `${percent}%` }} />
      </div>
      <div id="spectrum" className="spectrum" role="img" aria-label={t.spectrum}>
        {meter.bars.map((level, index) => (
          // oxlint-disable-next-line react/no-array-index-key -- the bars are a fixed row, never reordered
          <i key={index} style={{ blockSize: `${Math.round(level * 100)}%` }} />
        ))}
      </div>
      {clipping && (
        <p id="clipping" className="warning" role="alert">
          {t.clipping}
        </p>
      )}
    </div>
  );
}

type SentItem = Item & { kind: 'sent' };
type FileItem = Item & { kind: 'file' };

const isLive = (item: FileItem) => item.status === 'offered' || item.status === 'moving';

function fileNote(t: Messages, item: FileItem): string {
  const { outgoing, status, cast } = item;
  const slow = item.seconds !== undefined && item.seconds > SLOW_FILE;
  const parts = [t.fileSize(item.size)];
  if (status === 'offered') {
    parts.push(outgoing ? t.fileWaiting : t.fileOffered);
    if (slow) parts.push(t.fileSlow(item.seconds ?? 0));
  } else if (status === 'moving' && cast) {
    parts.push(t.broadcasting(cast.needed > 0 ? Math.floor((cast.symbols / cast.needed) * 100) : 0));
    if (slow && cast.symbols === 0) parts.push(t.fileSlow(item.seconds ?? 0));
  } else if (status === 'moving') {
    const percent = item.size > 0 ? Math.floor((item.done / item.size) * 100) : 0;
    parts.push(`${outgoing ? t.sending : t.fileReceiving} ${percent}%`);
    if (item.secondsLeft !== null) parts.push(t.timeLeft(item.secondsLeft));
  } else if (status === 'done') {
    // nobody tells a broadcast who has the file
    parts.push(cast ? t.sent : outgoing ? t.delivered : t.received);
  } else if (status === 'failed') {
    parts.push(outgoing ? t.notDelivered : t.fileNotReceived);
  } else if (status === 'refused') {
    parts.push(outgoing ? t.fileRefused : t.fileGivenUp);
  } else {
    parts.push(t.cancelled);
  }
  return parts.join(' · ');
}

interface FileRowProps {
  t: Messages;
  item: FileItem;
  onAccept: (item: FileItem) => void;
  onCancel: (item: FileItem) => void;
  onResend: (item: FileItem) => void;
}

function FileRow({ t, item, onAccept, onCancel, onResend }: FileRowProps) {
  const offer = !item.outgoing && item.status === 'offered';
  return (
    <li className={item.outgoing ? 'file sent' : 'file received'} data-status={item.status}>
      <span className="text">{item.name}</span>
      <small>{fileNote(t, item)}</small>
      {item.status === 'moving' &&
        (item.cast ? (
          <progress value={Math.min(item.cast.symbols, item.cast.needed)} max={Math.max(item.cast.needed, 1)} aria-label={item.name} />
        ) : (
          <progress value={item.done} max={item.size} aria-label={item.name} />
        ))}
      {offer && (
        <button type="button" className="accept" aria-label={t.accept} title={t.accept} onClick={() => onAccept(item)}>
          ✓
        </button>
      )}
      {isLive(item) && (
        <button
          type="button"
          className="cancel"
          aria-label={offer ? t.decline : t.cancel}
          title={offer ? t.decline : t.cancel}
          onClick={() => onCancel(item)}
        >
          ✕
        </button>
      )}
      {item.outgoing && !isLive(item) && item.status !== 'done' && (
        <button type="button" className="resend" aria-label={t.resend} title={t.resend} onClick={() => onResend(item)}>
          ↻
        </button>
      )}
      {item.url !== undefined && (
        <a className="save" href={item.url} download={item.name}>
          {t.save}
        </a>
      )}
    </li>
  );
}

interface FeedItemProps {
  t: Messages;
  item: Exclude<Item, FileItem>;
  onResend: (item: SentItem) => void;
  onCancel: (item: SentItem) => void;
}

function FeedItem({ t, item, onResend, onCancel }: FeedItemProps) {
  if (item.kind === 'lost') return <li className="note">{t.frameLost}</li>;
  if (item.kind === 'peer') return <li className="note peer">{item.name === '' ? t.peerAnonymous : t.peerNamed(item.name)}</li>;
  let note: string;
  if (item.kind === 'received') {
    note = `${t.received} · ${t.snr(item.snrDb)}`;
  } else {
    const status = { sending: t.sending, sent: t.sent, delivered: t.delivered, failed: t.notDelivered, cancelled: t.cancelled }[
      item.status
    ];
    note = item.status === 'sending' && item.attempt > 1 ? `${status} · ${t.attempt(item.attempt)}` : status;
  }
  return (
    <li className={item.kind} data-status={item.kind === 'sent' ? item.status : undefined}>
      <span className="text">{item.text}</span>
      <small>{note}</small>
      {item.kind === 'sent' && item.status === 'sending' && (
        <button type="button" className="cancel" aria-label={t.cancel} title={t.cancel} onClick={() => onCancel(item)}>
          ✕
        </button>
      )}
      {item.kind === 'sent' && (item.status === 'failed' || item.status === 'cancelled') && (
        <button type="button" className="resend" aria-label={t.resend} title={t.resend} onClick={() => onResend(item)}>
          ↻
        </button>
      )}
    </li>
  );
}

/**
 * The modem on the page: the station settings, the microphone switch, the
 * input meter, the messages and files, the text and the file to send.
 */
export function ModemPanel({ t, core, startAudio, fileUrl, coreReady, station, onStationChange }: ModemPanelProps) {
  const [link, setLink] = useState<Link>({ status: 'off' });
  const [items, setItems] = useState<Item[]>([]);
  const [transmitting, setTransmitting] = useState(false);
  /** Push-to-talk: the modem waits for the button. */
  const [pttWanted, setPttWanted] = useState(false);
  /** The speed texts go out at now, bit/s. */
  const [bitRate, setBitRate] = useState<number>(MODE_BIT_RATES[0]);
  const [draft, setDraft] = useState('');
  /** What the modem refused to take last. */
  const [sendFailed, setSendFailed] = useState<'text' | 'file' | null>(null);
  const nextKey = useRef(1);
  /** The number of the modem that runs: grows each time the microphone is turned on. */
  const run = useRef(0);
  /** The running audio device, for the clean-up when the panel goes away. */
  const running = useRef<AudioSession | null>(null);
  const feedBox = useRef<HTMLDivElement>(null);

  // The newest message is the one to see.
  useEffect(() => {
    const box = feedBox.current;
    if (box && items.length > 0) box.scrollTop = box.scrollHeight;
  }, [items.length]);

  useEffect(() => {
    const add = (item: Item) => setItems((list) => [...list, item]);
    const update = (id: number, change: (item: SentItem) => Item) =>
      setItems((list) => list.map((item) => (item.kind === 'sent' && item.run === run.current && item.id === id ? change(item) : item)));
    const isFile = (item: Item, id: number): item is FileItem => item.kind === 'file' && item.run === run.current && item.id === id;
    /** What the modem says about a file counts only while the file is on its way here. */
    const updateFile = (id: number, change: Partial<FileItem>) =>
      setItems((list) => list.map((item) => (isFile(item, id) && isLive(item) ? { ...item, ...change } : item)));
    return core.onEvent((event) => {
      const key = nextKey.current++;
      switch (event.kind) {
        case 'fileOffer': {
          const { id, name, size } = event;
          add({ key, kind: 'file', run: run.current, id, outgoing: false, name, size, done: 0, secondsLeft: null, status: 'offered' });
          break;
        }
        case 'fileHeard': {
          const { id, name, size } = event;
          add({ key, kind: 'file', run: run.current, id, outgoing: false, name, size, done: 0, secondsLeft: null, status: 'moving' });
          break;
        }
        case 'fileProgress':
          updateFile(event.id, { status: 'moving', done: event.done, secondsLeft: event.secondsLeft });
          break;
        case 'fileCast':
          updateFile(event.id, { cast: { symbols: event.symbols, needed: event.needed } });
          break;
        case 'fileSent':
          updateFile(event.id, { status: 'done' });
          break;
        case 'fileReceived': {
          const { id, name, bytes } = event;
          const whole = { status: 'done', size: bytes.length, done: bytes.length, url: fileUrl(bytes) } as const;
          // The messages may have been cleared meanwhile: the file is shown all the same.
          setItems((list) =>
            list.some((item) => isFile(item, id))
              ? list.map((item) => (isFile(item, id) ? { ...item, ...whole } : item))
              : [...list, { key, kind: 'file', run: run.current, id, outgoing: false, name, secondsLeft: null, ...whole }],
          );
          break;
        }
        case 'fileDelivered':
          setItems((list) =>
            list.map((item) => (isFile(item, event.id) && isLive(item) ? { ...item, status: 'done', done: item.size } : item)),
          );
          break;
        case 'fileFailed':
          updateFile(event.id, { status: 'failed' });
          break;
        case 'fileRefused':
          updateFile(event.id, { status: 'refused' });
          break;
        case 'message':
          add({ key, kind: 'received', text: event.text, snrDb: event.snrDb });
          break;
        case 'hello':
          add({ key, kind: 'peer', name: event.name });
          break;
        case 'frameLost':
          add({ key, kind: 'lost' });
          break;
        case 'delivered':
        case 'failed':
        case 'sent':
          update(event.id, (item) => ({ ...item, status: event.kind }));
          break;
        case 'retry':
          update(event.id, (item) => ({ ...item, attempt: event.attempt }));
          break;
        case 'txMode':
          setBitRate(event.bitRate);
          break;
        case 'txRequest':
          setPttWanted(true);
          break;
        case 'txStart':
          setPttWanted(false);
          setTransmitting(true);
          break;
        case 'txEnd':
          setTransmitting(false);
          break;
      }
    });
  }, [core, fileUrl]);

  useEffect(
    () => () => {
      running.current?.stop();
      if (running.current) void core.stop().catch(() => {});
      running.current = null;
    },
    [core],
  );

  const pressPtt = () => {
    setPttWanted(false);
    void core.ptt().catch(() => {});
  };

  // The space bar is the push-to-talk key, unless a text is being typed.
  useEffect(() => {
    if (!pttWanted) return undefined;
    const onKey = (event: KeyboardEvent) => {
      if (event.code !== 'Space' || event.target instanceof HTMLInputElement) return;
      event.preventDefault();
      setPttWanted(false);
      void core.ptt().catch(() => {});
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [pttWanted, core]);

  const turnOn = async () => {
    setLink({ status: 'starting' });
    let session: AudioSession | null = null;
    try {
      session = await startAudio();
      const { maxTextBytes } = await core.start(session.sampleRate, session.port, station);
      running.current = session;
      run.current++;
      setBitRate(MODE_BIT_RATES[station.mode ?? 0] ?? MODE_BIT_RATES[0]);
      setLink({ status: 'on', session, maxTextBytes, manualPtt: station.manualPtt, oneWay: station.oneWay });
    } catch (error) {
      session?.stop();
      // The audio device started but the modem did not: not the browser's fault.
      setLink({ status: 'error', kind: session ? 'failed' : audioErrorKind(error) });
    }
  };

  const turnOff = () => {
    running.current?.stop();
    running.current = null;
    void core.stop().catch(() => {});
    setTransmitting(false);
    setPttWanted(false);
    // What was still on its way will never be acknowledged now.
    setItems((list) =>
      list.map((item) => {
        if (item.kind === 'sent' && item.status === 'sending') return { ...item, status: 'failed' };
        return item.kind === 'file' && isLive(item) ? { ...item, status: 'failed' } : item;
      }),
    );
    setLink({ status: 'off' });
  };

  const text = draft.trim();
  const over = link.status === 'on' ? encoder.encode(text).length - link.maxTextBytes : 0;

  /** Queues a text in the modem and shows it; false when the modem refused. */
  const transmit = async (message: string, replaces?: number) => {
    setSendFailed(null);
    try {
      const { id } = await core.sendText(message);
      const item: Item = { key: nextKey.current++, kind: 'sent', run: run.current, id, text: message, status: 'sending', attempt: 1 };
      setItems((list) => [...list.filter((other) => other.key !== replaces), item]);
      return true;
    } catch {
      setSendFailed('text');
      return false;
    }
  };

  /** Offers a file to the other side, or broadcasts it, and shows it. */
  const offerFile = async (name: string, bytes: Uint8Array, replaces?: number) => {
    setSendFailed(null);
    // nobody is asked on a one-way channel: the file goes at once
    const broadcast = link.status === 'on' && link.oneWay;
    try {
      const { id, seconds } = await core.sendFile(name, bytes);
      const item: Item = {
        key: nextKey.current++,
        kind: 'file',
        run: run.current,
        id,
        outgoing: true,
        name,
        size: bytes.length,
        done: 0,
        secondsLeft: null,
        status: broadcast ? 'moving' : 'offered',
        bytes,
        seconds,
        ...(broadcast && { cast: { symbols: 0, needed: 0 } }),
      };
      setItems((list) => [...list.filter((other) => other.key !== replaces), item]);
    } catch {
      setSendFailed('file');
    }
  };

  const chooseFile = async (event: ChangeEvent<HTMLInputElement>) => {
    const input = event.target;
    const file = input.files?.[0];
    // the same file can be chosen again
    input.value = '';
    if (link.status !== 'on' || !file) return;
    try {
      await offerFile(file.name, new Uint8Array(await file.arrayBuffer()));
    } catch {
      setSendFailed('file');
    }
  };

  /** Consents to an offered file. */
  const acceptFile = async ({ key, id }: FileItem) => {
    try {
      await core.acceptFile(id);
    } catch {
      return;
    }
    setItems((list) =>
      list.map((item) => (item.key === key && item.kind === 'file' && item.status === 'offered' ? { ...item, status: 'moving' } : item)),
    );
  };

  /** Gives a file up, or declines its offer; what arrived whole meanwhile stays. */
  const cancelFile = async ({ key, id }: FileItem) => {
    try {
      await core.cancelFile(id);
    } catch {
      return;
    }
    setItems((list) =>
      list.map((item) => (item.key === key && item.kind === 'file' && isLive(item) ? { ...item, status: 'cancelled' } : item)),
    );
  };

  /** Takes a text back from the modem; what was delivered meanwhile stays delivered. */
  const cancel = async ({ key, id }: SentItem) => {
    try {
      await core.cancel(id);
    } catch {
      return;
    }
    setItems((list) =>
      list.map((item) => (item.key === key && item.kind === 'sent' && item.status === 'sending' ? { ...item, status: 'cancelled' } : item)),
    );
  };

  const send = async (event: FormEvent) => {
    event.preventDefault();
    if (link.status !== 'on' || text === '' || over > 0) return;
    setDraft('');
    if (!(await transmit(text))) setDraft(text);
  };

  const on = link.status === 'on';
  const stateText = () => {
    if (!transmitting) return t.listening;
    return on && link.manualPtt ? t.pttHold : t.transmitting;
  };

  return (
    <section className="modem">
      <fieldset className="station" disabled={on || link.status === 'starting'}>
        <input
          id="name"
          type="text"
          autoComplete="off"
          maxLength={MAX_NAME_LENGTH}
          aria-label={t.nameLabel}
          placeholder={t.nameLabel}
          value={station.name}
          onChange={(event) => onStationChange({ ...station, name: event.target.value })}
        />
        <label title={t.pttHint}>
          <input
            id="ptt-mode"
            type="checkbox"
            checked={station.manualPtt}
            onChange={(event) => onStationChange({ ...station, manualPtt: event.target.checked })}
          />
          {t.pttLabel}
        </label>
        <label title={t.oneWayHint}>
          <input
            id="one-way"
            type="checkbox"
            checked={station.oneWay}
            onChange={(event) => onStationChange({ ...station, oneWay: event.target.checked })}
          />
          {t.oneWayLabel}
        </label>
        <select
          id="mode"
          aria-label={t.speedLabel}
          value={station.mode ?? 'auto'}
          onChange={(event) => onStationChange({ ...station, mode: event.target.value === 'auto' ? null : Number(event.target.value) })}
        >
          <option value="auto">{t.speedAuto}</option>
          {MODE_BIT_RATES.map((rate, mode) => (
            <option key={rate} value={mode}>
              {t.bitRate(rate)}
            </option>
          ))}
        </select>
      </fieldset>

      <div className="link">
        {on ? (
          <>
            <div className="state-box">
              <span id="state" className={transmitting ? 'state transmitting' : 'state'} role="status">
                {stateText()}
              </span>
              <small id="speed">{t.bitRate(bitRate)}</small>
            </div>
            <button id="hello" type="button" className="secondary" onClick={() => void core.hello().catch(() => {})}>
              {t.hello}
            </button>
            <button id="mic" type="button" className="secondary" onClick={turnOff}>
              {t.micOff}
            </button>
          </>
        ) : (
          <button id="mic" type="button" disabled={!coreReady || link.status === 'starting'} onClick={() => void turnOn()}>
            {link.status === 'starting' ? t.micStarting : t.micOn}
          </button>
        )}
      </div>
      {link.status === 'error' && (
        <p id="mic-error" className="warning" role="alert">
          {errorText(t, link.kind)}
        </p>
      )}
      {on && pttWanted && (
        <div id="ptt" className="ptt" role="alert">
          <p>{t.pttPrompt}</p>
          <button id="ptt-button" type="button" onClick={pressPtt}>
            {t.pttButton}
          </button>
        </div>
      )}
      {on && <InputMeter t={t} session={link.session} />}

      <div className="feed-head">
        <h2>{t.messages}</h2>
        {items.length > 0 && (
          <button id="clear" type="button" className="secondary" onClick={() => setItems([])}>
            {t.clear}
          </button>
        )}
      </div>
      <div className="feed-box" ref={feedBox}>
        {items.length === 0 ? (
          <p className="empty">{t.noMessages}</p>
        ) : (
          <ol id="feed" className="feed" aria-live="polite">
            {items.map((item) =>
              item.kind === 'file' ? (
                <FileRow
                  key={item.key}
                  t={t}
                  item={item}
                  onAccept={(offered) => void acceptFile(offered)}
                  onCancel={(live) => void cancelFile(live)}
                  onResend={(stopped) => void (on && stopped.bytes && offerFile(stopped.name, stopped.bytes, stopped.key))}
                />
              ) : (
                <FeedItem
                  key={item.key}
                  t={t}
                  item={item}
                  onResend={(failed) => void (on && transmit(failed.text, failed.key))}
                  onCancel={(sending) => void cancel(sending)}
                />
              ),
            )}
          </ol>
        )}
      </div>

      {on && (
        <form className="composer" onSubmit={(event) => void send(event)}>
          <input
            id="draft"
            type="text"
            autoComplete="off"
            aria-label={t.messageLabel}
            placeholder={t.messageLabel}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
          <label className="attach" title={t.attach}>
            <input id="file" type="file" aria-label={t.attach} onChange={(event) => void chooseFile(event)} />
            <span aria-hidden="true">📎</span>
          </label>
          <button id="send" type="submit" disabled={text === '' || over > 0}>
            {t.send}
          </button>
          {over > 0 && (
            <p id="too-long" className="warning">
              {t.tooLong(over)}
            </p>
          )}
          {sendFailed && (
            <p id="send-failed" className="warning" role="alert">
              {sendFailed === 'file' ? t.fileNotSent : t.sendFailed}
            </p>
          )}
        </form>
      )}
    </section>
  );
}
