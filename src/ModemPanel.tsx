import { useEffect, useRef, useState, type FormEvent } from 'react';
import { audioErrorKind, type AudioErrorKind, type AudioSession, type StartAudio } from './audio/engine';
import { CLIP_LEVEL, SILENT, type Meter } from './audio/meter';
import type { Messages } from './i18n';
import { MAX_NAME_LENGTH } from './storage';
import type { CoreClient } from './worker/client';
import type { StationOptions } from './worker/protocol';

export interface ModemPanelProps {
  t: Messages;
  core: CoreClient;
  startAudio: StartAudio;
  /** False until the core has loaded: the microphone cannot be turned on. */
  coreReady: boolean;
  /** The name of the station and whether it is a push-to-talk radio; they take effect when the microphone is turned on. */
  station: StationOptions;
  onStationChange: (station: StationOptions) => void;
}

type Link =
  | { status: 'off' }
  | { status: 'starting' }
  | { status: 'on'; session: AudioSession; maxTextBytes: number; manualPtt: boolean }
  | { status: 'error'; kind: AudioErrorKind };

type Item =
  | { key: number; kind: 'received'; text: string; snrDb: number }
  /** `id` is the modem's; `attempt` grows with every repeat. */
  | { key: number; kind: 'sent'; id: number; text: string; status: 'sending' | 'sent' | 'delivered' | 'failed'; attempt: number }
  | { key: number; kind: 'peer'; name: string }
  | { key: number; kind: 'lost' };

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

function FeedItem({ t, item }: { t: Messages; item: Item }) {
  if (item.kind === 'lost') return <li className="note">{t.frameLost}</li>;
  if (item.kind === 'peer') return <li className="note peer">{item.name === '' ? t.peerAnonymous : t.peerNamed(item.name)}</li>;
  let note: string;
  if (item.kind === 'received') {
    note = `${t.received} · ${t.snr(item.snrDb)}`;
  } else {
    const status = { sending: t.sending, sent: t.sent, delivered: t.delivered, failed: t.notDelivered }[item.status];
    note = item.status === 'sending' && item.attempt > 1 ? `${status} · ${t.attempt(item.attempt)}` : status;
  }
  return (
    <li className={item.kind} data-status={item.kind === 'sent' ? item.status : undefined}>
      <span className="text">{item.text}</span>
      <small>{note}</small>
    </li>
  );
}

/** The modem on the page: the station settings, the microphone switch, the input meter, the messages and the text to send. */
export function ModemPanel({ t, core, startAudio, coreReady, station, onStationChange }: ModemPanelProps) {
  const [link, setLink] = useState<Link>({ status: 'off' });
  const [items, setItems] = useState<Item[]>([]);
  const [transmitting, setTransmitting] = useState(false);
  /** Push-to-talk: the modem waits for the button. */
  const [pttWanted, setPttWanted] = useState(false);
  const [draft, setDraft] = useState('');
  const [sendFailed, setSendFailed] = useState(false);
  const nextKey = useRef(1);
  /** The running audio device, for the clean-up when the panel goes away. */
  const running = useRef<AudioSession | null>(null);

  useEffect(() => {
    const add = (item: Item) => setItems((list) => [...list, item]);
    const update = (id: number, change: (item: Item & { kind: 'sent' }) => Item) =>
      setItems((list) => list.map((item) => (item.kind === 'sent' && item.id === id ? change(item) : item)));
    return core.onEvent((event) => {
      const key = nextKey.current++;
      switch (event.kind) {
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
  }, [core]);

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
      setLink({ status: 'on', session, maxTextBytes, manualPtt: station.manualPtt });
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
    setItems((list) => list.map((item) => (item.kind === 'sent' && item.status === 'sending' ? { ...item, status: 'failed' } : item)));
    setLink({ status: 'off' });
  };

  const text = draft.trim();
  const over = link.status === 'on' ? encoder.encode(text).length - link.maxTextBytes : 0;

  const send = async (event: FormEvent) => {
    event.preventDefault();
    if (link.status !== 'on' || text === '' || over > 0) return;
    setSendFailed(false);
    setDraft('');
    try {
      const { id } = await core.sendText(text);
      setItems((list) => [...list, { key: nextKey.current++, kind: 'sent', id, text, status: 'sending', attempt: 1 }]);
    } catch {
      setDraft(text);
      setSendFailed(true);
    }
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
        <label>
          <input
            id="ptt-mode"
            type="checkbox"
            checked={station.manualPtt}
            onChange={(event) => onStationChange({ ...station, manualPtt: event.target.checked })}
          />
          {t.pttLabel}
        </label>
      </fieldset>

      <div className="link">
        {on ? (
          <>
            <span id="state" className={transmitting ? 'state transmitting' : 'state'} role="status">
              {stateText()}
            </span>
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

      <h2>{t.messages}</h2>
      {items.length === 0 ? (
        <p className="empty">{t.noMessages}</p>
      ) : (
        <ol id="feed" className="feed" aria-live="polite">
          {items.map((item) => (
            <FeedItem key={item.key} t={t} item={item} />
          ))}
        </ol>
      )}

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
              {t.sendFailed}
            </p>
          )}
        </form>
      )}
    </section>
  );
}
