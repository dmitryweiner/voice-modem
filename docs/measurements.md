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

## Mode R1 through speech codecs (2026-11-10)

`node scripts/codecs.mjs` — five messages of 2 to 255 bytes (64 s) through
each codec with ffmpeg 8 and back, no other impairment. The SNR is what the
receiver estimated: here it measures the distortion by the codec.

| Codec                  | Messages | SNR, dB (min … max) |
| ---------------------- | -------- | ------------------- |
| none (16-bit WAV)      | 5/5      | 25.9 … 39.3         |
| G.711 µ-law, 64 kbit/s | 5/5      | 23.9 … 29.0         |
| G.726, 32 kbit/s       | 5/5      | 18.5 … 20.5         |
| G.726, 16 kbit/s       | 5/5      | 9.8 … 10.8          |
| iLBC, 15.2 kbit/s      | **3/5**  | −4.6 … −2.9         |
| G.723.1, 6.3 kbit/s    | 5/5      | 6.0 … 6.9           |
| Opus voice, 24 kbit/s  | 5/5      | 14.8 … 20.5         |
| Opus voice, 16 kbit/s  | 5/5      | 11.0 … 12.4         |
| Opus voice, 12 kbit/s  | 5/5      | 3.1 … 3.7           |
| Opus voice, 8 kbit/s   | 5/5      | −5.0 … 4.2          |
| Opus voice, 6 kbit/s   | 5/5      | −5.0 … 4.2          |

What this says about the main risk of the plan (vocoders leaving no usable
channel):

- 62.5 bit/s gets through every codec tried, down to 6 kbit/s, with one
  exception.
- **iLBC loses two of five messages**, and loses them whole — the preamble or
  the header, not the payload — although the three it passes arrive with the
  payload intact. Not investigated yet; the first thing to look at when the
  telephone preset is tuned (stage 6).
- **AMR and EVS, the codecs of cellular calls, were not tested**: this ffmpeg
  build has no AMR encoder. G.723.1 (the same family of coders, at 6.3 kbit/s)
  is the nearest stand-in here. The real answer comes from a real call
  (stage 2 onwards) or a build with `libopencore_amrnb`, which the script
  picks up by itself.
- No test here has packet loss, time stretching by a jitter buffer, noise
  suppression or a voice detector — what messengers and phones add on top of
  the codec.

## Speed of the receiver

Searching for a preamble in noise: 131× real time natively on an M-series Mac
(release build). Not yet measured as WASM in a browser.
