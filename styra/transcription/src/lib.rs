//! Speech-to-text for audio an operator hands to Styra.
//!
//! Transcription is done in the calling process, on a local Whisper model
//! through `whisper.cpp`. It deliberately does not go to an agent provider: a
//! transcript is a mechanical transformation of the operator's own microphone,
//! it should not spend an interactive quota, and it should not need a sandbox,
//! credentials, or a network round trip to produce.
//!
//! It is a crate of its own rather than a module of the server because it
//! shares nothing with the session runner but the request that asks for it,
//! and because it carries a model runtime and an audio decoder that nothing
//! else in Styra has any use for. Builds are CPU-only unless the `vulkan` or
//! `rocm` feature selects a whisper.cpp GPU backend.
//!
//! The model is large enough that loading it is the expensive part, so one
//! context is loaded on first use and kept for the life of the process.
//!
//! Transcription never downloads anything. The weights are fetched once, by
//! `styra-transcribe --download-model`, and a request made before that fails
//! saying so. An operator who presses record and waits should be waiting on
//! their own machine, not on a multi-gigabyte download they did not ask for
//! and cannot see the progress of from inside the editor.

use anyhow::{bail, Context, Result};
use hf_hub::{api::sync::Api, Cache};
use rodio::source::UniformSourceIterator;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const MODEL_REPOSITORY: &str = "ggerganov/whisper.cpp";
const MODEL_VARIABLE: &str = "STYRA_WHISPER_MODEL";
const DEVICE_VARIABLE: &str = "STYRA_WHISPER_DEVICE";

/// The model used unless `STYRA_WHISPER_MODEL` names another.
///
/// Large-v3-turbo gives substantially better dictation than `small`, while its
/// Q5 weights are only modestly larger and fit comfortably in the shared
/// memory of the AMD iGPU this is built for. It remains usable as a CPU
/// fallback without carrying the full model's 1.6 GB footprint.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Model {
    name: &'static str,
    filename: &'static str,
}

/// A deliberately finite list: accepting an arbitrary string would turn a
/// typo into a network request and a misleading missing-file error.
const MODELS: &[Model] = &[
    Model {
        name: "tiny",
        filename: "ggml-tiny.bin",
    },
    Model {
        name: "tiny.en",
        filename: "ggml-tiny.en.bin",
    },
    Model {
        name: "tiny-q5_1",
        filename: "ggml-tiny-q5_1.bin",
    },
    Model {
        name: "tiny-q8_0",
        filename: "ggml-tiny-q8_0.bin",
    },
    Model {
        name: "tiny.en-q5_1",
        filename: "ggml-tiny.en-q5_1.bin",
    },
    Model {
        name: "tiny.en-q8_0",
        filename: "ggml-tiny.en-q8_0.bin",
    },
    Model {
        name: "base",
        filename: "ggml-base.bin",
    },
    Model {
        name: "base.en",
        filename: "ggml-base.en.bin",
    },
    Model {
        name: "base-q5_1",
        filename: "ggml-base-q5_1.bin",
    },
    Model {
        name: "base-q8_0",
        filename: "ggml-base-q8_0.bin",
    },
    Model {
        name: "base.en-q5_1",
        filename: "ggml-base.en-q5_1.bin",
    },
    Model {
        name: "base.en-q8_0",
        filename: "ggml-base.en-q8_0.bin",
    },
    Model {
        name: "small",
        filename: "ggml-small.bin",
    },
    Model {
        name: "small.en",
        filename: "ggml-small.en.bin",
    },
    Model {
        name: "small-q5_1",
        filename: "ggml-small-q5_1.bin",
    },
    Model {
        name: "small-q8_0",
        filename: "ggml-small-q8_0.bin",
    },
    Model {
        name: "small.en-q5_1",
        filename: "ggml-small.en-q5_1.bin",
    },
    Model {
        name: "small.en-q8_0",
        filename: "ggml-small.en-q8_0.bin",
    },
    Model {
        name: "medium",
        filename: "ggml-medium.bin",
    },
    Model {
        name: "medium.en",
        filename: "ggml-medium.en.bin",
    },
    Model {
        name: "medium-q5_0",
        filename: "ggml-medium-q5_0.bin",
    },
    Model {
        name: "medium-q8_0",
        filename: "ggml-medium-q8_0.bin",
    },
    Model {
        name: "medium.en-q5_0",
        filename: "ggml-medium.en-q5_0.bin",
    },
    Model {
        name: "medium.en-q8_0",
        filename: "ggml-medium.en-q8_0.bin",
    },
    Model {
        name: "large-v1",
        filename: "ggml-large-v1.bin",
    },
    Model {
        name: "large-v2",
        filename: "ggml-large-v2.bin",
    },
    Model {
        name: "large-v2-q5_0",
        filename: "ggml-large-v2-q5_0.bin",
    },
    Model {
        name: "large-v2-q8_0",
        filename: "ggml-large-v2-q8_0.bin",
    },
    Model {
        name: "large-v3",
        filename: "ggml-large-v3.bin",
    },
    Model {
        name: "large-v3-q5_0",
        filename: "ggml-large-v3-q5_0.bin",
    },
    Model {
        name: "large-v3-turbo",
        filename: "ggml-large-v3-turbo.bin",
    },
    Model {
        name: "large-v3-turbo-q8_0",
        filename: "ggml-large-v3-turbo-q8_0.bin",
    },
];

struct Transcriber {
    context: WhisperContext,
}

/// whisper.cpp contexts are reusable, and serializing the states also avoids
/// two requests competing for the same GPU and shared-memory bandwidth.
static TRANSCRIBER: OnceLock<Mutex<Transcriber>> = OnceLock::new();

/// The verbatim English transcript of `path`, with no surrounding whitespace.
///
/// The decoder accepts every format enabled on `rodio`; its output is converted
/// to the mono, 16 kHz floating-point PCM expected by whisper.cpp.
pub fn transcribe(path: &Path) -> Result<String> {
    let audio = path
        .canonicalize()
        .with_context(|| format!("resolving audio file {}", path.display()))?;
    if !audio.is_file() {
        bail!("audio path is not a regular file: {}", audio.display());
    }
    let file = std::fs::File::open(&audio)
        .with_context(|| format!("opening audio file {}", audio.display()))?;
    let decoded = rodio::Decoder::new(BufReader::new(file))
        .with_context(|| format!("decoding audio file {}", audio.display()))?;
    let samples: Vec<f32> = UniformSourceIterator::new(decoded, 1, 16_000).collect();
    if samples.is_empty() {
        bail!("the audio file contained no samples");
    }

    let transcriber = transcriber()?;
    let transcriber = transcriber
        .lock()
        .map_err(|_| anyhow::anyhow!("the transcription model is poisoned"))?;
    let mut state = transcriber
        .context
        .create_state()
        .context("creating a Whisper transcription state")?;
    let mut parameters = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    parameters.set_language(Some("en"));
    parameters.set_translate(false);
    parameters.set_print_special(false);
    parameters.set_print_progress(false);
    parameters.set_print_realtime(false);
    parameters.set_print_timestamps(false);
    state
        .full(parameters, &samples)
        .context("running the Whisper transcription model")?;

    let transcript = state
        .as_iter()
        .map(|segment| segment.to_string())
        .collect::<String>();
    let transcript = transcript.trim().to_owned();
    if transcript.is_empty() {
        bail!("the audio contained no recognisable speech");
    }
    Ok(transcript)
}

/// Fetch the configured GGML model into the Hugging Face cache.
///
/// Re-running this once the model is present is a no-op. `hf-hub` downloads to
/// a temporary file and publishes the completed, content-addressed cache entry
/// only after the transfer finishes.
pub fn download() -> Result<()> {
    let model = model()?;
    if local_model(model).is_some() {
        println!("whisper model {} is already downloaded", model.name);
        return Ok(());
    }
    println!("downloading whisper model {}…", model.name);
    let path = Api::new()
        .context("opening the Hugging Face model cache")?
        .model(MODEL_REPOSITORY.to_owned())
        .get(model.filename)
        .with_context(|| format!("downloading the whisper model {}", model.name))?;
    println!(
        "whisper model {} is ready at {}",
        model.name,
        path.display()
    );
    Ok(())
}

/// The process-wide model, loaded on first use.
///
/// A failure is not cached, so installing a missing driver or downloading the
/// model is enough for the next request to retry without restarting the server.
fn transcriber() -> Result<&'static Mutex<Transcriber>> {
    if let Some(ready) = TRANSCRIBER.get() {
        return Ok(ready);
    }
    let model = model()?;
    let path = local_model(model).with_context(|| {
        format!(
            "the whisper model {} is not downloaded: run `styra-transcribe --download-model` first",
            model.name
        )
    })?;
    // whisper.cpp is exceptionally chatty in debug builds (down to every
    // decoded token). Styra reports the useful lifecycle itself; route the
    // native logs through whisper-rs' no-op hooks instead of flooding the
    // server log or a standalone command's stderr.
    whisper_rs::install_logging_hooks();
    let mut parameters = WhisperContextParameters::default();
    parameters.use_gpu(device()?.uses_gpu()?);
    let context = WhisperContext::new_with_params(&path, parameters).with_context(|| {
        format!(
            "loading the whisper transcription model {} from {}",
            model.name,
            path.display()
        )
    })?;
    Ok(TRANSCRIBER.get_or_init(|| Mutex::new(Transcriber { context })))
}

fn local_model(model: Model) -> Option<PathBuf> {
    Cache::default()
        .model(MODEL_REPOSITORY.to_owned())
        .get(model.filename)
}

fn model() -> Result<Model> {
    configured_model(std::env::var(MODEL_VARIABLE).ok().as_deref())
}

fn configured_model(name: Option<&str>) -> Result<Model> {
    let name = name.or(Some("quantized_medium")).unwrap();
    // Preserve the spellings accepted by the rwhisper implementation so an
    // existing service environment keeps selecting the same class of model.
    let canonical = match name {
        "quantized_tiny" => "tiny-q5_1",
        "quantized_tiny_en" => "tiny.en-q5_1",
        "quantized_base" => "base-q8_1",
        "quantized_small" => "small-q8_1",
        "quantized_medium" => "medium-q8_0",
        "quantized_large_v3_turbo" => "large-v3-turbo-q5_0",
        "large" => "large-v1",
        other => other,
    };
    let canonical = canonical
        .replace('_', "-")
        .replace("q5-", "q5_")
        .replace("q8-", "q8_");
    MODELS
        .iter()
        .copied()
        .find(|model| model.name == canonical)
        .ok_or_else(|| anyhow::anyhow!("{MODEL_VARIABLE}={name:?} is not a Whisper model"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Device {
    Auto,
    Cpu,
    Gpu,
}

fn device() -> Result<Device> {
    configured_device(std::env::var(DEVICE_VARIABLE).ok().as_deref())
}

fn configured_device(name: Option<&str>) -> Result<Device> {
    match name.map(str::trim).filter(|name| !name.is_empty()) {
        None | Some("auto") => Ok(Device::Auto),
        Some("cpu") => Ok(Device::Cpu),
        Some("gpu" | "vulkan" | "rocm") => Ok(Device::Gpu),
        Some(name) => {
            bail!("{DEVICE_VARIABLE}={name:?} is not a transcription device; use auto, cpu, or gpu")
        }
    }
}

impl Device {
    fn uses_gpu(self) -> Result<bool> {
        let accelerated = cfg!(any(feature = "vulkan", feature = "rocm"));
        match self {
            Device::Auto => Ok(accelerated && gpu_available()),
            Device::Cpu => Ok(false),
            Device::Gpu if accelerated && gpu_available() => Ok(true),
            Device::Gpu if accelerated => {
                bail!("{DEVICE_VARIABLE}=gpu requested acceleration, but no GPU backend device is available")
            }
            Device::Gpu => bail!(
                "{DEVICE_VARIABLE}=gpu requested acceleration, but this binary was built without the `vulkan` or `rocm` feature"
            ),
        }
    }
}

fn gpu_available() -> bool {
    // whisper-rs exposes Vulkan enumeration, but the underlying C++ helper can
    // throw across the FFI boundary when no render device exists, which aborts
    // the Rust process. A usable Linux render node is the safe prerequisite;
    // context creation below remains the authoritative driver/backend check.
    #[cfg(all(feature = "vulkan", target_os = "linux"))]
    if std::fs::read_dir("/dev/dri").is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let render_node = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("renderD"));
            render_node
                && std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(entry.path())
                    .is_ok()
        })
    }) {
        return true;
    }
    #[cfg(all(feature = "vulkan", not(target_os = "linux")))]
    return true;
    // HIP device enumeration is not exposed by whisper-rs. Model creation
    // remains the authoritative check and returns an error if ROCm cannot
    // initialize a device.
    #[cfg(feature = "rocm")]
    return !cfg!(target_os = "linux")
        || std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kfd")
            .is_ok();
    #[allow(unreachable_code)]
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_override_accepts_native_and_legacy_spellings() {
        assert_eq!(configured_model(None).unwrap(), configured_model(Some("quantized_base")).unwrap());
        assert_eq!(
            configured_model(Some("quantized_tiny")).unwrap().name,
            "tiny-q5_1"
        );
        assert_eq!(
            configured_model(Some("large_v3_turbo")).unwrap().name,
            "large-v3-turbo"
        );
        assert!(configured_model(Some("not-a-model")).is_err());
    }

    #[test]
    fn device_override_is_strict() {
        assert_eq!(configured_device(None).unwrap(), Device::Auto);
        assert_eq!(configured_device(Some("cpu")).unwrap(), Device::Cpu);
        assert_eq!(configured_device(Some("vulkan")).unwrap(), Device::Gpu);
        assert!(configured_device(Some("magic")).is_err());
    }

    #[test]
    fn a_missing_audio_file_is_reported_by_path() {
        let missing = std::env::temp_dir().join("styra-transcribe-absent.wav");
        std::fs::remove_file(&missing).ok();
        let error = transcribe(&missing).unwrap_err();
        assert!(
            error.to_string().contains("resolving audio file"),
            "{error:#}"
        );
    }
}
