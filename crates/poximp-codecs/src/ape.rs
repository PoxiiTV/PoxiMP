//! Monkey's Audio (.ape) as a symphonia format. `ape-decoder` decompresses one APE frame per
//! packet and the packet carries plain PCM, so symphonia's own PCM decoder does the rest.

use std::io::Cursor;

use ape_decoder::ApeDecoder;
use symphonia::core::audio::Channels;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_PCM_S16LE, CODEC_ID_PCM_S24LE, CODEC_ID_PCM_S32LE, CODEC_ID_PCM_S8, CODEC_ID_PCM_U8,
};
use symphonia::core::codecs::audio::AudioCodecParameters;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::common::FourCc;
use symphonia::core::errors::{seek_error, unsupported_error, Error, Result, SeekErrorKind};
use symphonia::core::formats::prelude::*;
use symphonia::core::formats::probe::{ProbeDataMatchSpec, ProbeFormatData, ProbeableFormat, Score, Scoreable};
use symphonia::core::io::{MediaSourceStream, ScopedStream};
use symphonia::core::meta::{Metadata, MetadataLog};

const INFO: FormatInfo =
    FormatInfo { format: FormatId::new(FourCc::new(*b"MAC ")), short_name: "ape", long_name: "Monkey's Audio" };

pub struct ApeReader<'s> {
    dec: ApeDecoder<MediaSourceStream<'s>>,
    media_info: MediaInfo,
    tracks: Vec<Track>,
    metadata: MetadataLog,
    tb: TimeBase,
    next_frame: u32,
}

impl<'s> ApeReader<'s> {
    fn try_new(mss: MediaSourceStream<'s>, opts: FormatOptions) -> Result<Self> {
        let dec = ApeDecoder::new(mss).map_err(|_| Error::Unsupported("ape: invalid header"))?;
        let i = dec.info();
        if i.is_big_endian || i.is_floating_point || i.channels == 0 || i.sample_rate == 0 {
            return unsupported_error("ape: sample format");
        }
        let codec = match i.bits_per_sample {
            8 if i.is_signed_8bit => CODEC_ID_PCM_S8,
            8 => CODEC_ID_PCM_U8,
            16 => CODEC_ID_PCM_S16LE,
            24 => CODEC_ID_PCM_S24LE,
            32 => CODEC_ID_PCM_S32LE,
            _ => return unsupported_error("ape: bit depth"),
        };
        let tb = TimeBase::try_from_recip(i.sample_rate).ok_or(Error::Unsupported("ape: sample rate"))?;
        let mut params = AudioCodecParameters::new();
        params
            .for_codec(codec)
            .with_sample_rate(i.sample_rate)
            .with_channels(Channels::Discrete(i.channels))
            .with_bits_per_sample(u32::from(i.bits_per_sample))
            .with_bits_per_coded_sample(u32::from(i.bits_per_sample))
            .with_max_frames_per_packet(u64::from(i.blocks_per_frame));
        let mut track = Track::new(0);
        track
            .with_codec_params(CodecParameters::Audio(params))
            .with_time_base(tb)
            .with_num_frames(i.total_samples)
            .with_duration(Duration::from(i.total_samples));
        Ok(Self {
            media_info: MediaInfo::from_track(&track),
            tracks: vec![track],
            metadata: opts.external_data.metadata.unwrap_or_default(),
            dec,
            tb,
            next_frame: 0,
        })
    }
}

impl Scoreable for ApeReader<'_> {
    fn score(_src: ScopedStream<&mut MediaSourceStream<'_>>) -> Result<Score> {
        Ok(Score::Supported(255))
    }
}

impl ProbeableFormat<'_> for ApeReader<'_> {
    fn try_probe_new(mss: MediaSourceStream<'_>, opts: FormatOptions) -> Result<Box<dyn FormatReader + '_>> {
        Ok(Box::new(ApeReader::try_new(mss, opts)?))
    }

    fn probe_data() -> &'static [ProbeFormatData] {
        &[ProbeFormatData {
            spec: ProbeDataMatchSpec { extensions: &["ape"], mime_types: &["audio/ape", "audio/x-ape"], markers: &[b"MAC "] },
            info: INFO,
        }]
    }
}

impl FormatReader for ApeReader<'_> {
    fn format_info(&self) -> &FormatInfo {
        &INFO
    }

    fn media_info(&self) -> &MediaInfo {
        &self.media_info
    }

    fn metadata(&mut self) -> Metadata<'_> {
        self.metadata.metadata()
    }

    fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Lands on the start of the APE frame holding the target: callers trim to the exact sample.
    fn seek(&mut self, _mode: SeekMode, to: SeekTo) -> Result<SeekedTo> {
        let i = self.dec.info();
        let required_ts = match to {
            SeekTo::Timestamp { ts, .. } => ts,
            SeekTo::Time { time, .. } => self.tb.calc_timestamp(time)
                .ok_or(Error::SeekError(SeekErrorKind::OutOfRange))?,
        };
        if required_ts.get() < 0 || required_ts.get() as u64 >= i.total_samples.max(1) {
            return seek_error(SeekErrorKind::OutOfRange);
        }
        self.next_frame = frame_of(required_ts.get() as u64, i.blocks_per_frame, i.total_frames);
        let actual_ts = Timestamp::new(i64::from(self.next_frame) * i64::from(i.blocks_per_frame));
        Ok(SeekedTo { track_id: 0, required_ts, actual_ts })
    }

    fn next_packet(&mut self) -> Result<Option<Packet>> {
        let (frame, per_frame) = (self.next_frame, self.dec.info().blocks_per_frame);
        if frame >= self.dec.total_frames() {
            return Ok(None);
        }
        let pcm = self.dec.decode_frame(frame).map_err(|_| Error::DecodeError("ape: corrupt frame"))?;
        self.next_frame += 1;
        let frames = pcm.len() as u64 / u64::from(self.dec.info().block_align.max(1));
        let ts = Timestamp::new(i64::from(frame) * i64::from(per_frame));
        Ok(Some(Packet::new(0, ts, Duration::from(frames), pcm)))
    }

    /// ponytail: the APE decoder owns the stream and cannot hand it back; nothing in PoxiMP
    /// chains formats, so an empty stream is returned. Upgrade if ape-decoder grows into_inner.
    fn into_inner<'s>(self: Box<Self>) -> MediaSourceStream<'s>
    where
        Self: 's,
    {
        MediaSourceStream::new(Box::new(Cursor::new(Vec::new())), Default::default())
    }
}

/// APE frame holding sample `ts` (frames are `per_frame` samples, the last one shorter).
fn frame_of(ts: u64, per_frame: u32, total_frames: u32) -> u32 {
    ((ts / u64::from(per_frame.max(1))) as u32).min(total_frames.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_lands_on_the_frame_start() {
        assert_eq!(frame_of(0, 73_728, 10), 0);
        assert_eq!(frame_of(73_727, 73_728, 10), 0);
        assert_eq!(frame_of(73_728, 73_728, 10), 1);
        assert_eq!(frame_of(u64::MAX, 73_728, 10), 9, "clamped to the last frame");
        assert_eq!(frame_of(5, 0, 0), 0, "degenerate headers never divide by zero");
    }
}
