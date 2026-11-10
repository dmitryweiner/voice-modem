import { useEffect, useRef, useState, type FormEvent } from 'react';
import { audioErrorKind, type AudioErrorKind, type AudioSession, type StartAudio } from './audio/engine';
import { CLIP_LEVEL, SILENT, type Meter } from './audio/meter';
import type { Messages } from './i18n';
import type { CoreClient } from './worker/client';

export interface ModemPanelProps {
  t: Messages;
  core: CoreClient;
  startAudio: StartAudio;
  /** False until the core has loaded: the microphone cannot be turned on. */
  coreReady: boolean;
}

type Link =
  | { status: 'off' }
  | { status: 'starting' }
  | { status: 'on'; session: AudioSession; maxTextBytes: number }
  | { status: 'error'; kind: AudioErrorKind };

type Item =
  | { id: number; kind: 'received'; text: string; snrDb: number }
  | { id: number; kind: 'sent'; text: string; seconds: number; done: boolean }
  | { id: number; kind: 'lost' };

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
  if (item.kind === 'lost') return <li className="lost">{t.frameLost}</li>;
  const note =
    item.kind === 'received' ? `${t.received} · ${t.snr(item.snrDb)}` : `${item.done ? t.sent : t.sending} · ${t.seconds(item.seconds)}`;
  return (
    <li className={item.kind}>
      <span className="text">{item.text}</span>
      <small>{note}</small>
    </li>
  );
}

/** The modem on the page: the microphone switch, the input meter, the messages and the text to send. */
export function ModemPanel({ t, core, startAudio, coreReady }: ModemPanelProps) {
  const [link, setLink] = useState<Link>({ status: 'off' });
  const [items, setItems] = useState<Item[]>([]);
  const [transmitting, setTransmitting] = useState(false);
  const [draft, setDraft] = useState('');
  const [sendFailed, setSendFailed] = useState(false);
  const nextId = useRef(1);
  /** The running audio device, for the clean-up when the panel goes away. */
  const running = useRef<AudioSession | null>(null);

  useEffect(
    () =>
      core.onEvent((event) => {
        if (event.kind === 'txEnd') {
          setTransmitting(false);
          setItems((list) => list.map((item) => (item.kind === 'sent' ? { ...item, done: true } : item)));
          return;
        }
        const id = nextId.current++;
        const item: Item = event.kind === 'message' ? { id, kind: 'received', text: event.text, snrDb: event.snrDb } : { id, kind: 'lost' };
        setItems((list) => [...list, item]);
      }),
    [core],
  );

  useEffect(
    () => () => {
      running.current?.stop();
      if (running.current) void core.stop().catch(() => {});
      running.current = null;
    },
    [core],
  );

  const turnOn = async () => {
    setLink({ status: 'starting' });
    let session: AudioSession | null = null;
    try {
      session = await startAudio();
      const { maxTextBytes } = await core.start(session.sampleRate, session.port);
      running.current = session;
      setLink({ status: 'on', session, maxTextBytes });
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
    setLink({ status: 'off' });
  };

  const text = draft.trim();
  const over = link.status === 'on' ? encoder.encode(text).length - link.maxTextBytes : 0;

  const send = async (event: FormEvent) => {
    event.preventDefault();
    if (link.status !== 'on' || transmitting || text === '' || over > 0) return;
    setTransmitting(true);
    setSendFailed(false);
    try {
      const { seconds } = await core.sendText(text);
      setItems((list) => [...list, { id: nextId.current++, kind: 'sent', text, seconds, done: false }]);
      setDraft('');
    } catch {
      setTransmitting(false);
      setSendFailed(true);
    }
  };

  return (
    <section className="modem">
      <div className="link">
        {link.status === 'on' ? (
          <>
            <span id="state" className={transmitting ? 'state transmitting' : 'state'} role="status">
              {transmitting ? t.transmitting : t.listening}
            </span>
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
      {link.status === 'on' && <InputMeter t={t} session={link.session} />}

      <h2>{t.messages}</h2>
      {items.length === 0 ? (
        <p className="empty">{t.noMessages}</p>
      ) : (
        <ol id="feed" className="feed" aria-live="polite">
          {items.map((item) => (
            <FeedItem key={item.id} t={t} item={item} />
          ))}
        </ol>
      )}

      {link.status === 'on' && (
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
          <button id="send" type="submit" disabled={transmitting || text === '' || over > 0}>
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
