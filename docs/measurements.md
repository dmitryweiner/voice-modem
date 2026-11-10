# Measurements

What the modem withstands, measured — first in simulation, later on real
channels. Each table says how to repeat it.

SNR is the ratio of the signal power to the power of white noise over the
whole band up to 8 kHz; in a 3 kHz band the same noise is 4.3 dB weaker.

## Mode R1 in simulation (2026-11-10)

`cargo run --release -p modem-sim --example sweep` — 40-byte frames (6.7 s),
50 per point.

| White noise, SNR | Frames received |
| ---------------- | --------------- |
| −15 dB and above | 50/50           |
| −16 dB           | 46/50           |
| −17 dB           | 26/50           |
| −18 dB           | 1/50            |

- **Frequency offset** (at −10 dB): received 50/50 at every offset tried from
  −250 to +250 Hz; the estimate is within 0.2 Hz on average.
- **Sound-card clock offset**, the longest frame (255 bytes, 34 s), no noise:
  received up to ±500 ppm, lost at ±700 ppm. The receiver does not track the
  symbol clock yet, so shorter frames tolerate proportionally more.
- **False frames**: none in 10 minutes of pure noise.
- No frame with a wrong payload was ever delivered: beyond the limit frames
  are lost or reported as damaged.

`crates/modem-sim/tests/link.rs` guards these with about 3 dB of margin, plus
a telephone band with echoes of 12–55 ms, clipping of a ten times overdriven
signal and a signal 60 dB down.

## Mode R1 in a room, simulated (2026-11-11)

The same sweep. The room is a direct sound followed by a dense tail of
reflections that decays by 60 dB in the time RT60 (`Reverb` in `modem-sim`);
"direct" is the energy of the direct sound over that of all reflections — it
falls by 6 dB with every doubling of the distance. Noise is 20 dB below the
signal: it plays no part here. 40-byte frames, 25 rooms per point.

| RT60  | Direct | One set of tones (stage 3) | Four sets in turn |
| ----- | ------ | -------------------------- | ----------------- |
| 0.3 s | 0 dB   | 24/25                      | 25/25             |
| 0.3 s | −3 dB  | 12/25                      | 25/25             |
| 0.3 s | −6 dB  | 4/25                       | 25/25             |
| 0.3 s | −9 dB  | 1/25                       | 25/25             |
| 0.3 s | −12 dB | 3/25                       | 12/25             |
| 0.5 s | 0 dB   | 24/25                      | 25/25             |
| 0.5 s | −3 dB  | 11/25                      | 25/25             |
| 0.5 s | −6 dB  | 1/25                       | 25/25             |
| 0.5 s | −9 dB  | 0/25                       | 24/25             |
| 0.5 s | −12 dB | 1/25                       | 14/25             |

- With one set of tones the modem stopped where the reflections equal the
  direct sound, and the SNR it reported there was −6 to −8 dB — the numbers
  seen in the real room at 3 m just before it stopped (below). The "noise" it
  measured was its own echo.
- Four sets of tones used in turn move the limit by 9 dB, which in a room is
  nearly three times the distance. Whether a real room agrees is to be
  measured.
- Other things unchanged by the four sets: white noise (50/50 at −15 dB),
  frequency offsets, the codecs below. One thing improved by accident: the
  34 s frame now survives a clock offset of ±1000 ppm (it was ±500), because
  a window that has slipped onto the previous symbol finds no energy in its
  own tones instead of a wrong tone.
- Beyond −9 dB the preamble is the next thing to fail.

## Mode R1 through speech codecs (2026-11-10)

`node scripts/codecs.mjs` — five messages of 2 to 255 bytes (64 s) through
each codec with ffmpeg 8 and back, no other impairment. The SNR is what the
receiver estimated: here it measures the distortion by the codec.

| Codec                  | Messages | SNR, dB (min … max) |
| ---------------------- | -------- | ------------------- |
| none (16-bit WAV)      | 5/5      | 25.8 … 32.1         |
| G.711 µ-law, 64 kbit/s | 5/5      | 24.2 … 27.4         |
| G.726, 32 kbit/s       | 5/5      | 18.6 … 20.5         |
| G.726, 16 kbit/s       | 5/5      | 10.2 … 11.6         |
| iLBC, 15.2 kbit/s      | **4/5**  | −11.0 … −1.2        |
| G.723.1, 6.3 kbit/s    | 5/5      | 5.9 … 7.3           |
| AMR-NB, 12.2 kbit/s    | 5/5      | 11.6 … 12.8         |
| AMR-NB, 4.75 kbit/s    | 5/5      | 3.0 … 3.9           |
| Opus voice, 24 kbit/s  | 5/5      | 8.0 … 20.8          |
| Opus voice, 16 kbit/s  | 5/5      | 10.7 … 13.0         |
| Opus voice, 12 kbit/s  | 5/5      | 3.5 … 5.1           |
| Opus voice, 8 kbit/s   | 5/5      | 3.4 … 4.7           |
| Opus voice, 6 kbit/s   | 5/5      | 3.4 … 4.5           |

Measured with the frames of stage 3: one-way (broadcast) frames with the
7-byte header, messages cut into segments of 63 bytes — the 255-byte message
is five frames, all of which must arrive.

What this says about the main risk of the plan (vocoders leaving no usable
channel):

- 62.5 bit/s gets through every codec tried, down to 4.75 kbit/s, with one
  exception. **AMR-NB**, the codec of GSM and 3G calls, passes everything at
  both its highest and its lowest rate.
- **iLBC loses one of five messages** (two of five with the frames of stage
  1), and loses frames whole — the preamble or the header, not the payload.
  Not investigated yet; the first thing to look at when the
  telephone preset is tuned (stage 6).
- AMR-WB and EVS (the codecs of VoLTE calls) were not tested: ffmpeg has no
  encoder for them.
- The codec alone is not a call. No test here has discontinuous transmission
  (the voice detector that mutes "silence"), noise suppression, packet loss
  or time stretching by a jitter buffer — what phones and messengers add on
  top. The real answer comes from a real call.

## Speed of the receiver (2026-11-10)

M-series Mac. Searching for a preamble in noise:

| Build           | Device rate | Speed          |
| --------------- | ----------- | -------------- |
| native, release | 16 kHz      | 131× real time |
| WASM in Node 24 | 16 kHz      | 128× real time |
| WASM in Node 24 | 48 kHz      | 95× real time  |
| WASM in Node 24 | 44.1 kHz    | 97× real time  |

At 48 kHz the sample rate converter is a quarter of the work. With a sine
computed per filter tap it was four fifths (22× real time); it now reads the
filter from a table.

The worst case is not noise but **a frame whose beginning was missed**: its
tones cross the preamble threshold about 60 times a second, and each crossing
starts a search for the exact peak and the second chirp. Native: 18× real
time, 36× since that search looks at every fourth position first. Not yet
measured on a phone, where it matters.

## Over the air between real devices (2026-11-10)

The stage 2 build (one-way text, no repeats), by the author, in a room:

| From → to                                   | Result               |
| ------------------------------------------- | -------------------- |
| MacBook Pro → the same MacBook, another tab | received, SNR +10 dB |
| Android phone (Chrome) → MacBook Pro        | received, SNR +12 dB |
| MacBook Pro → Android phone (Chrome)        | received, SNR +7 dB  |

- **Range: about 4 m.** Beyond it nothing is received, and "heard but could
  not be read" never appeared — at that time it was shown only for a bad
  payload, so the loss is in the preamble or the header.
- The SNR at arm's length is 15–20 dB below what the same signal gives
  through a cable (above 26 dB), and 22–27 dB above where white noise stops
  the modem. So it is hardly noise that limits the range. The suspects are
  what a room and small speakers do and white noise does not: echoes longer
  than a symbol, and distortion of the chirps. To be found out before any
  tuning; unreadable headers after a clear preamble are now reported, which
  tells the two apart.
- iPhone and a phone call in between: not tried yet.

The stage 3 build (acknowledgements and repeats), same room, MacBook and
Android phone: texts are delivered and acknowledged both ways, greetings and
statuses work. Received texts show SNR −2, −4, −6 dB as the distance grows;
**at about 3 m** frames are "heard but could not be read" and nothing gets
through. The simulated room above gives the same picture, which is what
pointed at reverberation; the four sets of tones are the answer to it and
have not been tried in the room yet.

On a Mac the display must be awake for a browser to process sound at all:
with the display asleep an AudioContext reports "running" while its clock
stands still. `npm run smoke` keeps it awake with `caffeinate`.
