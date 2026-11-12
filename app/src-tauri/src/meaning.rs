//! Turns text into numbers with the Mac's own language model. It's built in, works
//! offline and nothing leaves the device, so search by meaning needs no AI key.

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple {
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::Message;
    use objc2_foundation::NSString;
    use objc2_natural_language::{
        NLEmbedding, NLLanguage, NLLanguageEnglish, NLLanguageRecognizer,
    };
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    struct Model(Option<Retained<NLEmbedding>>);
    // Only ever touched while holding the lock below, so one thread at a time.
    unsafe impl Send for Model {}

    // Loading a model takes a moment, so each is kept once loaded, for every thread to share.
    static MODELS: LazyLock<Mutex<HashMap<String, Model>>> = LazyLock::new(Default::default);

    fn language_of(text: &NSString) -> Option<Retained<NLLanguage>> {
        unsafe {
            NLLanguageRecognizer::dominantLanguageForString(text)
                .or_else(|| NLLanguageEnglish.map(|l| l.retain()))
        }
    }

    pub fn embed(text: &str) -> Option<Vec<f32>> {
        if text.trim().is_empty() {
            return None;
        }
        // These threads last, so what the Mac hands back has to be let go of here.
        autoreleasepool(|_| {
            let text = NSString::from_str(text);
            let language = language_of(&text)?;
            let mut models = MODELS.lock().unwrap_or_else(|e| e.into_inner());
            let model = models.entry(language.to_string()).or_insert_with(|| {
                Model(unsafe { NLEmbedding::sentenceEmbeddingForLanguage(&language) })
            });
            let vector = unsafe { model.0.as_ref()?.vectorForString(&text)? };
            Some(vector.iter().map(|n| n.floatValue()).collect())
        })
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use apple::embed;

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub fn embed(_text: &str) -> Option<Vec<f32>> {
    None
}

#[cfg(test)]
mod tests {
    use super::embed;

    fn likeness(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let size = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (size(a) * size(b))
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn similar_sentences_come_out_close_on_this_mac() {
        let Some(flight) = embed("Your flight to Paris leaves at noon tomorrow") else {
            // A Mac without the English model can't test this, and search still works.
            return;
        };
        let plane = embed("Boarding pass for your plane to France").unwrap();
        let invoice = embed("Please pay the attached invoice by Friday").unwrap();
        assert!(!flight.is_empty());
        assert!(
            likeness(&flight, &plane) > likeness(&flight, &invoice),
            "travel should be closer to travel than to billing"
        );
    }
}
