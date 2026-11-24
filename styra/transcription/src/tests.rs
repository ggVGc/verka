use super::*;

#[test]
fn model_override_accepts_native_and_legacy_spellings() {
    // Every legacy spelling names a model that is actually in the catalog.
    for legacy in [
        "quantized_tiny",
        "quantized_tiny_en",
        "quantized_base",
        "quantized_small",
        "quantized_medium",
        "quantized_large_v3_turbo",
        "large",
    ] {
        configured_model(Some(legacy))
            .unwrap_or_else(|error| panic!("legacy spelling {legacy:?}: {error}"));
    }
    // An unset override still selects a model, and underscores in a native
    // name are normalized to the catalog's dashes.
    configured_model(None).unwrap();
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
