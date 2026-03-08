---
name: voice
description: OminiX ASR (speech-to-text), TTS (text-to-speech), and model management via Qwen3 models on Apple Silicon. Triggers: voice, transcribe audio, text to speech, model management, download model, 语音识别, 语音合成, speak this, read aloud, 模型管理.
version: 1.0.0
author: hagency
always: false
---

# OminiX ASR / TTS / Model Management

On-device speech-to-text, text-to-speech, and model management using OminiX Qwen3 ASR/TTS models on Apple Silicon.

## Configuration

The asr skill auto-discovers the ominix-api server URL via (in priority order):
1. `OMINIX_API_URL` environment variable
2. Discovery file `~/.ominix/api_url` (written by ominix-api on startup)
3. Default: `http://localhost:8080`

## Tools

### voice_transcribe

Transcribe an audio file to text. Supports WAV, OGG, MP3, FLAC, M4A.

```json
{"audio_path": "/tmp/voice.ogg", "language": "Chinese"}
```

**Parameters:**
- `audio_path` (required): Absolute path to the audio file
- `language` (optional, default "Chinese"): "Chinese", "English", "Japanese", "Korean", "Cantonese", etc.

### voice_synthesize

Generate speech audio from text. Produces a WAV file.

```json
{"text": "Hello world", "language": "english", "speaker": "vivian"}
```

**Parameters:**
- `text` (required): Text to synthesize
- `output_path` (optional): Where to save WAV. Default: `/tmp/crew_tts_<timestamp>.wav`
- `language` (optional, default "chinese"): "chinese", "english", "japanese", "korean"
- `speaker` (optional, default "vivian"): Voice preset name

**Available speakers:** vivian, serena, ryan, aiden, eric, dylan (English/Chinese), uncle_fu (Chinese), ono_anna (Japanese), sohee (Korean)

### list_models

List all loaded models and available models in the catalog.

```json
{}
```

No parameters required. Returns loaded models and downloadable catalog.

### download_model

Download a model from the catalog.

```json
{"model_id": "Qwen3-ASR-2B-MLX-4bit"}
```

**Parameters:**
- `model_id` (required): Model ID from the catalog (use list_models to see available models)

### load_model

Load a downloaded model into memory for inference.

```json
{"model": "Qwen3-ASR-2B-MLX-4bit", "model_type": "asr"}
```

**Parameters:**
- `model` (required): Model name or path
- `model_type` (optional, default "llm"): "llm", "asr", "tts"

### unload_model

Unload a model from memory.

```json
{"model_type": "asr"}
```

**Parameters:**
- `model_type` (required): Type of model to unload — "llm", "asr", "tts"
