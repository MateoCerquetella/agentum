//! Speech model catalog — a direct port of orca's `model-catalog.ts`.
//!
//! Each entry describes an on-device sherpa-onnx ASR model: where to download
//! it, how to verify it, which ONNX files it ships, and how the recognizer must
//! be configured (streaming vs offline, model family, modeling unit). The
//! renderer renders this list verbatim in Settings → Voice, so the serialized
//! field names must stay camelCase to match `shared/speech-types.ts`.

use serde::Serialize;

/// Model family. Decides which recognizer the engine builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechModelType {
    Transducer,
    Paraformer,
    Whisper,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechModelManifest {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    #[serde(rename = "type")]
    pub model_type: SpeechModelType,
    pub language: &'static str,
    pub size_bytes: u64,
    pub download_url: &'static str,
    pub archive_sha256: &'static str,
    pub archive_format: &'static str,
    pub files: &'static [&'static str],
    pub sample_rate: u32,
    pub streaming: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modeling_unit: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended: Option<bool>,
}

pub const SPEECH_MODEL_CATALOG: &[SpeechModelManifest] = &[
    SpeechModelManifest {
        id: "parakeet-tdt-0.6b-v3-int8",
        label: "Parakeet TDT v3",
        description: "Highest accuracy for 25 European languages. Punctuation, capitalization, and word-level timestamps.",
        model_type: SpeechModelType::Transducer,
        language: "multilingual",
        size_bytes: 180_000_000,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
        archive_sha256: "5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf",
        archive_format: "tar.bz2",
        files: &["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"],
        sample_rate: 16000,
        streaming: false,
        modeling_unit: Some("bpe"),
        recommended: Some(true),
    },
    SpeechModelManifest {
        id: "parakeet-tdt-0.6b-v2-int8",
        label: "Parakeet TDT v2",
        description: "English only. Faster than v3 with similar accuracy. Punctuation and capitalization.",
        model_type: SpeechModelType::Transducer,
        language: "en",
        size_bytes: 170_000_000,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2",
        archive_sha256: "157c157bc51155e03e37d2466522a3a737dd9c72bb25f36eb18912964161e1ad",
        archive_format: "tar.bz2",
        files: &["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"],
        sample_rate: 16000,
        streaming: false,
        modeling_unit: Some("bpe"),
        recommended: None,
    },
    SpeechModelManifest {
        id: "zipformer-bilingual-zh-en",
        label: "Zipformer Bilingual",
        description: "Chinese + English with code-switching. Low-latency real-time streaming.",
        model_type: SpeechModelType::Transducer,
        language: "zh-en",
        size_bytes: 130_000_000,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20.tar.bz2",
        archive_sha256: "27ffbd9ee24ad186d99acc2f6354d7992b27bcab490812510665fa8f9389c5f8",
        archive_format: "tar.bz2",
        files: &[
            "encoder-epoch-99-avg-1.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.onnx",
            "tokens.txt",
        ],
        sample_rate: 16000,
        streaming: true,
        modeling_unit: Some("cjkchar+bpe"),
        recommended: None,
    },
    SpeechModelManifest {
        id: "paraformer-bilingual-zh-en",
        label: "Paraformer Bilingual",
        description: "Chinese (Mandarin + dialects) + English. Strong on accented and regional Chinese.",
        model_type: SpeechModelType::Paraformer,
        language: "zh-en",
        size_bytes: 115_000_000,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-paraformer-bilingual-zh-en.tar.bz2",
        archive_sha256: "5462a1fce42693deae572af1e8c4687124b12aa85fe61ff4d3168bb5280e205f",
        archive_format: "tar.bz2",
        files: &["encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"],
        sample_rate: 16000,
        streaming: true,
        modeling_unit: None,
        recommended: None,
    },
    SpeechModelManifest {
        id: "zipformer-streaming-en-20m",
        label: "Zipformer Streaming EN",
        description: "English only. Lightweight 20M-param model, good balance of speed and size.",
        model_type: SpeechModelType::Transducer,
        language: "en",
        size_bytes: 128_000_000,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17.tar.bz2",
        archive_sha256: "9c559283e8498d3fe95913c79ca1cb454bb26281ac2b102b41306c7d752765d9",
        archive_format: "tar.bz2",
        files: &[
            "encoder-epoch-99-avg-1.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.onnx",
            "tokens.txt",
        ],
        sample_rate: 16000,
        streaming: true,
        modeling_unit: Some("bpe"),
        recommended: None,
    },
    SpeechModelManifest {
        id: "zipformer-streaming-zh-14m",
        label: "Zipformer Streaming ZH",
        description: "Chinese only. Ultra-lightweight 14M-param model, ideal for low-resource devices.",
        model_type: SpeechModelType::Transducer,
        language: "zh",
        size_bytes: 74_000_000,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-zh-14M-2023-02-23.tar.bz2",
        archive_sha256: "2cbd71b640d9c37d3784f29367333a4577b0398b62e9deeed418170b081cba8b",
        archive_format: "tar.bz2",
        files: &[
            "encoder-epoch-99-avg-1.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.onnx",
            "tokens.txt",
        ],
        sample_rate: 16000,
        streaming: true,
        modeling_unit: Some("cjkchar"),
        recommended: None,
    },
    SpeechModelManifest {
        id: "whisper-tiny",
        label: "Whisper Tiny",
        description: "90+ languages. Lower accuracy than Parakeet but broadest language coverage.",
        model_type: SpeechModelType::Whisper,
        language: "multilingual",
        size_bytes: 116_000_000,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-tiny.tar.bz2",
        archive_sha256: "c46116994e539aa165266d96b325252728429c12535eb9d8b6a2b10f129e66b1",
        archive_format: "tar.bz2",
        files: &["tiny-encoder.onnx", "tiny-decoder.onnx", "tiny-tokens.txt"],
        sample_rate: 16000,
        streaming: false,
        modeling_unit: None,
        recommended: None,
    },
];

pub fn get_catalog_model(id: &str) -> Option<&'static SpeechModelManifest> {
    SPEECH_MODEL_CATALOG.iter().find(|m| m.id == id)
}
