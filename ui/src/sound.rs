//! Small synthesized game sounds; no assets, external requests or script eval.

use chess_core::chess::MoveOutcome;
use js_sys::{Array, Function, Reflect};
use std::cell::RefCell;
use wasm_bindgen::{JsCast, JsValue};

const STORAGE_KEY: &str = "freechess:sound:v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    Move,
    Capture,
    Check,
    End,
}

struct Audio {
    context: Option<JsValue>,
    enabled: bool,
}

thread_local! {
    static AUDIO: RefCell<Audio> = RefCell::new(Audio {
        context: None,
        enabled: storage()
            .and_then(|s| s.get_item(STORAGE_KEY).ok().flatten())
            .as_deref() != Some("off"),
    });
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

pub fn enabled() -> bool {
    AUDIO.with(|audio| audio.borrow().enabled)
}

pub fn set_enabled(enabled: bool) {
    AUDIO.with(|audio| audio.borrow_mut().enabled = enabled);
    if let Some(storage) = storage() {
        let _ = storage.set_item(STORAGE_KEY, if enabled { "on" } else { "off" });
    }
    if enabled {
        unlock();
    }
}

/// Call directly from a pointer or keyboard event so browser autoplay permits audio.
pub fn unlock() {
    AUDIO.with(|audio| {
        let mut audio = audio.borrow_mut();
        if !audio.enabled {
            return;
        }
        if audio.context.is_none() {
            let global = js_sys::global();
            let constructor = ["AudioContext", "webkitAudioContext"]
                .into_iter()
                .find_map(|name| get(&global, name).ok()?.dyn_into::<Function>().ok());
            audio.context = constructor
                .and_then(|constructor| Reflect::construct(&constructor, &Array::new()).ok());
        }
        if let Some(context) = &audio.context {
            resume(context);
        }
    });
}

pub fn for_move(outcome: &MoveOutcome, ended: bool) -> Sound {
    if ended {
        Sound::End
    } else if outcome.gives_check {
        Sound::Check
    } else if outcome.captured.is_some() {
        Sound::Capture
    } else {
        Sound::Move
    }
}

pub fn play(sound: Sound) {
    AUDIO.with(|audio| {
        let audio = audio.borrow();
        if !audio.enabled {
            return;
        }
        let Some(context) = &audio.context else {
            return;
        };
        resume(context);
        let notes: &[(f64, f64, f64)] = match sound {
            Sound::Move => &[(280.0, 0.0, 0.07)],
            Sound::Capture => &[(150.0, 0.0, 0.11), (95.0, 0.04, 0.09)],
            Sound::Check => &[(660.0, 0.0, 0.13), (880.0, 0.12, 0.18)],
            Sound::End => &[
                (523.25, 0.0, 0.2),
                (659.25, 0.14, 0.2),
                (783.99, 0.28, 0.35),
            ],
        };
        for &(frequency, delay, duration) in notes {
            let _ = tone(context, frequency, delay, duration);
        }
    });
}

fn get(target: &JsValue, name: &str) -> Result<JsValue, JsValue> {
    Reflect::get(target, &JsValue::from_str(name))
}

fn call(target: &JsValue, name: &str, values: &[JsValue]) -> Result<JsValue, JsValue> {
    let function = get(target, name)?.dyn_into::<Function>()?;
    let args = Array::new();
    for value in values {
        args.push(value);
    }
    function.apply(target, &args)
}

fn resume(context: &JsValue) {
    if get(context, "state")
        .ok()
        .and_then(|state| state.as_string())
        .as_deref()
        != Some("suspended")
    {
        return;
    }
    if let Ok(promise) =
        call(context, "resume", &[]).and_then(|value| value.dyn_into::<js_sys::Promise>())
    {
        wasm_bindgen_futures::spawn_local(async move {
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        });
    }
}

fn tone(context: &JsValue, frequency: f64, delay: f64, duration: f64) -> Result<(), JsValue> {
    let start = get(context, "currentTime")?.as_f64().unwrap_or(0.0) + delay;
    let oscillator = call(context, "createOscillator", &[])?;
    let gain = call(context, "createGain", &[])?;
    Reflect::set(&oscillator, &"type".into(), &"triangle".into())?;
    let pitch = get(&oscillator, "frequency")?;
    call(&pitch, "setValueAtTime", &[frequency.into(), start.into()])?;
    call(
        &pitch,
        "exponentialRampToValueAtTime",
        &[(frequency * 0.65).into(), (start + duration).into()],
    )?;
    let volume = get(&gain, "gain")?;
    call(&volume, "setValueAtTime", &[0.0001.into(), start.into()])?;
    call(
        &volume,
        "linearRampToValueAtTime",
        &[0.12.into(), (start + 0.005).into()],
    )?;
    call(
        &volume,
        "exponentialRampToValueAtTime",
        &[0.0001.into(), (start + duration).into()],
    )?;
    call(&oscillator, "connect", std::slice::from_ref(&gain))?;
    call(&gain, "connect", &[get(context, "destination")?])?;
    let finished_oscillator = oscillator.clone();
    let finished = wasm_bindgen::closure::Closure::once_into_js(move || {
        let _ = call(&finished_oscillator, "disconnect", &[]);
        let _ = call(&gain, "disconnect", &[]);
    });
    Reflect::set(&oscillator, &"onended".into(), &finished)?;
    call(&oscillator, "start", &[start.into()])?;
    call(&oscillator, "stop", &[(start + duration + 0.01).into()])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::chess::{Game, Move};

    #[test]
    fn sounds_follow_legal_move_outcomes_and_end_overrides_check() {
        let mut game = Game::new();
        let quiet = game.play(Move::from_uci("e2e4").unwrap()).unwrap();
        assert_eq!(for_move(quiet, false), Sound::Move);
        game.play(Move::from_uci("d7d5").unwrap()).unwrap();
        let capture = game.play(Move::from_uci("e4d5").unwrap()).unwrap();
        assert_eq!(for_move(capture, false), Sound::Capture);
        game.play(Move::from_uci("d8d5").unwrap()).unwrap();
        game.play(Move::from_uci("b1c3").unwrap()).unwrap();
        let check = game.play(Move::from_uci("d5e5").unwrap()).unwrap();
        assert_eq!(for_move(check, false), Sound::Check);
        assert_eq!(for_move(check, true), Sound::End);
    }
}
