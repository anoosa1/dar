//! Discord voice: AudioWorklet mic capture, WebCodecs Opus encode/decode,
//! live WebSocket per Discord account, voice members, dock, stage and E2EE call-code dialog.

pub mod dock;
pub mod stage;

pub(super) use dock::{Controls, Marks};
pub use dock::{VoiceDock, VoiceMembers};
pub use stage::{CallCodeDialog, VoiceStage};

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use js_sys::{Array, Object, Reflect};
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::model::{
    BASE, ChatAccount, ChatLiveMessage, ChatVoiceCall, ChatVoiceServers, search_params,
};
use crate::api::{self, BrowserSession};
use crate::ui::window;

const MIC_WORKLET_URL: &str = "/rust/mic-worklet.js";

fn js_get(obj: &JsValue, prop: &str) -> JsValue {
    Reflect::get(obj, &JsValue::from_str(prop)).unwrap_or(JsValue::UNDEFINED)
}

fn js_set(obj: &JsValue, prop: &str, val: &JsValue) {
    let _ = Reflect::set(obj, &JsValue::from_str(prop), val);
}

fn js_obj(entries: &[(&str, JsValue)]) -> JsValue {
    let obj = Object::new();
    for (k, v) in entries {
        let _ = Reflect::set(&obj, &JsValue::from_str(k), v);
    }
    obj.into()
}

fn js_construct(name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let ctor = Reflect::get(&js_sys::global(), &JsValue::from_str(name))?;
    let func: js_sys::Function = ctor.dyn_into()?;
    let arr = Array::new();
    for a in args {
        arr.push(a);
    }
    Reflect::construct(&func, &arr)
}

fn js_call(obj: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let func: js_sys::Function = Reflect::get(obj, &JsValue::from_str(method))?.dyn_into()?;
    let arr = Array::new();
    for a in args {
        arr.push(a);
    }
    Reflect::apply(&func, obj, &arr)
}

fn has_global(name: &str) -> bool {
    !Reflect::get(&js_sys::global(), &JsValue::from_str(name))
        .unwrap_or(JsValue::UNDEFINED)
        .is_undefined()
}

fn perf_now() -> f64 {
    window()
        .performance()
        .map(|p| p.now())
        .unwrap_or_else(js_sys::Date::now)
}

pub fn voice_supported() -> bool {
    let nav: JsValue = window().navigator().into();
    let media_devices = js_get(&nav, "mediaDevices");
    has_global("AudioEncoder")
        && has_global("AudioDecoder")
        && has_global("AudioWorkletNode")
        && !media_devices.is_null()
        && !media_devices.is_undefined()
}

/// One Discord account's voice: who is where, its call, and whether this tab carries the call's audio.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountVoice {
    pub servers: ChatVoiceServers,
    pub call: Option<ChatVoiceCall>,
    pub here: bool,
    pub elsewhere: bool,
    pub online: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicState {
    Idle,
    On,
    Blocked,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EndedNotice {
    pub account_id: String,
    pub message: String,
    pub at: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VoiceSnapshot {
    pub accounts: BTreeMap<String, AccountVoice>,
    pub speaking: BTreeSet<String>,
    pub mic: MicState,
    pub ended: Option<EndedNotice>,
}

impl Default for VoiceSnapshot {
    fn default() -> Self {
        Self {
            accounts: BTreeMap::new(),
            speaking: BTreeSet::new(),
            mic: if voice_supported() {
                MicState::Idle
            } else {
                MicState::Unsupported
            },
            ended: None,
        }
    }
}

struct DecoderEntry {
    decoder: JsValue,
    _next: Rc<Cell<f64>>,
    _output: Closure<dyn FnMut(JsValue)>,
    _error: Closure<dyn FnMut(JsValue)>,
}

struct VoiceAudioInner {
    ctx: Option<JsValue>,
    master: Option<JsValue>,
    stream: Option<JsValue>,
    encoder: Option<JsValue>,
    _worklet_node: Option<JsValue>,
    _worklet_onmessage: Option<Closure<dyn FnMut(web_sys::MessageEvent)>>,
    _encoder_output: Option<Closure<dyn FnMut(JsValue)>>,
    _encoder_error: Option<Closure<dyn FnMut(JsValue)>>,
    decoders: BTreeMap<String, DecoderEntry>,
    heard: BTreeMap<String, f64>,
    muted: bool,
    deaf: bool,
    talking: bool,
    hang: u32,
    previous: Option<js_sys::Float32Array>,
    time: f64,
    timer: Option<i32>,
    _timer_cb: Option<Closure<dyn FnMut()>>,
    shown: String,
    send: Option<Rc<dyn Fn(Vec<u8>)>>,
    me: String,
    on_speaking: Rc<dyn Fn(BTreeSet<String>)>,
}

fn stop_stream_tracks(stream: &JsValue) {
    if let Ok(tracks) = js_call(stream, "getTracks", &[])
        && let Ok(arr) = tracks.dyn_into::<Array>()
    {
        for i in 0..arr.length() {
            let track = arr.get(i);
            let _ = js_call(&track, "stop", &[]);
        }
    }
}

impl VoiceAudioInner {
    fn new(me: String, on_speaking: Rc<dyn Fn(BTreeSet<String>)>) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self {
            ctx: None,
            master: None,
            stream: None,
            encoder: None,
            _worklet_node: None,
            _worklet_onmessage: None,
            _encoder_output: None,
            _encoder_error: None,
            decoders: BTreeMap::new(),
            heard: BTreeMap::new(),
            muted: true,
            deaf: false,
            talking: false,
            hang: 0,
            previous: None,
            time: 0.0,
            timer: None,
            _timer_cb: None,
            shown: String::new(),
            send: None,
            me,
            on_speaking,
        }))
    }

    async fn open(audio: Rc<RefCell<Self>>) -> MicState {
        let existing = audio.borrow().ctx.clone();
        if let Some(ctx) = existing {
            if let Ok(promise) = js_call(&ctx, "resume", &[])
                && let Ok(promise) = promise.dyn_into::<js_sys::Promise>()
            {
                let _ = JsFuture::from(promise).await;
            }
            return if audio.borrow().encoder.is_some() {
                MicState::On
            } else {
                MicState::Blocked
            };
        }

        let opts = js_obj(&[
            ("sampleRate", 48000.0.into()),
            ("latencyHint", "interactive".into()),
        ]);
        let Ok(ctx) = js_construct("AudioContext", &[opts]) else {
            return MicState::Blocked;
        };
        let Ok(master) = js_call(&ctx, "createGain", &[]) else {
            return MicState::Blocked;
        };
        let dest = js_get(&ctx, "destination");
        let _ = js_call(&master, "connect", &[dest]);
        {
            let mut inner = audio.borrow_mut();
            let gain_val = if inner.deaf { 0.0 } else { 1.0 };
            js_set(&js_get(&master, "gain"), "value", &gain_val.into());
            inner.ctx = Some(ctx.clone());
            inner.master = Some(master);
            let weak = Rc::downgrade(&audio);
            let cb = Closure::<dyn FnMut()>::new(move || {
                if let Some(a) = weak.upgrade() {
                    Self::tick(&a);
                }
            });
            if let Ok(id) = window().set_interval_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                100,
            ) {
                inner.timer = Some(id);
                inner._timer_cb = Some(cb);
            }
        }
        if let Ok(promise) = js_call(&ctx, "resume", &[])
            && let Ok(promise) = promise.dyn_into::<js_sys::Promise>()
        {
            let _ = JsFuture::from(promise).await;
        }

        let setup = async {
            let worklet = js_get(&ctx, "audioWorklet");
            let add_p = js_call(&worklet, "addModule", &[MIC_WORKLET_URL.into()])?
                .dyn_into::<js_sys::Promise>()?;
            JsFuture::from(add_p).await?;
            if audio.borrow().ctx.is_none() {
                return Err(JsValue::from_str("closed"));
            }
            let nav: JsValue = window().navigator().into();
            let devices = js_get(&nav, "mediaDevices");
            let audio_opts = js_obj(&[
                ("echoCancellation", true.into()),
                ("noiseSuppression", true.into()),
                ("autoGainControl", true.into()),
                ("channelCount", 1.into()),
            ]);
            let constraints = js_obj(&[("audio", audio_opts)]);
            let gum_p =
                js_call(&devices, "getUserMedia", &[constraints])?.dyn_into::<js_sys::Promise>()?;
            let stream = JsFuture::from(gum_p).await?;
            if audio.borrow().ctx.is_none() {
                stop_stream_tracks(&stream);
                return Err(JsValue::from_str("closed"));
            }
            audio.borrow_mut().stream = Some(stream.clone());

            let source = js_call(&ctx, "createMediaStreamSource", &[stream])?;
            let node_opts = js_obj(&[("numberOfInputs", 1.into()), ("numberOfOutputs", 1.into())]);
            let node = js_construct(
                "AudioWorkletNode",
                &[ctx.clone(), "dar-mic".into(), node_opts],
            )?;
            let silent = js_call(&ctx, "createGain", &[])?;
            js_set(&js_get(&silent, "gain"), "value", &0.0.into());
            js_call(&source, "connect", &[node.clone()])?;
            js_call(&node, "connect", &[silent.clone()])?;
            let dest = js_get(&ctx, "destination");
            js_call(&silent, "connect", &[dest])?;

            let weak_msg = Rc::downgrade(&audio);
            let onmsg = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
                move |event: web_sys::MessageEvent| {
                    if let Some(a) = weak_msg.upgrade()
                        && let Ok(samples) = event.data().dyn_into::<js_sys::Float32Array>()
                    {
                        Self::frame(&a, samples);
                    }
                },
            );
            let port = js_get(&node, "port");
            js_set(&port, "onmessage", onmsg.as_ref().unchecked_ref());

            let weak_enc = Rc::downgrade(&audio);
            let enc_out = Closure::<dyn FnMut(JsValue)>::new(move |chunk: JsValue| {
                let Some(a) = weak_enc.upgrade() else { return };
                let send_fn = a.borrow().send.clone();
                let Some(send_fn) = send_fn else { return };
                let byte_len = js_get(&chunk, "byteLength").as_f64().unwrap_or(0.0) as u32;
                let frame = js_sys::Uint8Array::new_with_length(1 + byte_len);
                frame.set_index(0, 2);
                let sub = frame.subarray(1, 1 + byte_len);
                let _ = js_call(&chunk, "copyTo", &[sub.into()]);
                send_fn(frame.to_vec());
            });
            let enc_err = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| {});
            let enc_init = js_obj(&[
                ("output", enc_out.as_ref().clone()),
                ("error", enc_err.as_ref().clone()),
            ]);
            let encoder = js_construct("AudioEncoder", &[enc_init])?;
            let opus_cfg = js_obj(&[("frameDuration", 20000.into())]);
            let enc_cfg = js_obj(&[
                ("codec", "opus".into()),
                ("sampleRate", 48000.into()),
                ("numberOfChannels", 1.into()),
                ("bitrate", 64000.into()),
                ("opus", opus_cfg),
            ]);
            js_call(&encoder, "configure", &[enc_cfg])?;

            let mut inner = audio.borrow_mut();
            inner.encoder = Some(encoder);
            inner._worklet_node = Some(node);
            inner._worklet_onmessage = Some(onmsg);
            inner._encoder_output = Some(enc_out);
            inner._encoder_error = Some(enc_err);
            Ok::<(), JsValue>(())
        }
        .await;

        match setup {
            Ok(()) => MicState::On,
            Err(_) => {
                let stream = audio.borrow_mut().stream.take();
                if let Some(s) = stream {
                    stop_stream_tracks(&s);
                }
                MicState::Blocked
            }
        }
    }

    fn set(audio: &Rc<RefCell<Self>>, muted: bool, deaf: bool) {
        let mut inner = audio.borrow_mut();
        inner.muted = muted || deaf;
        inner.deaf = deaf;
        if let Some(master) = &inner.master {
            js_set(
                &js_get(master, "gain"),
                "value",
                &(if deaf { 0.0 } else { 1.0 }).into(),
            );
        }
        if inner.muted {
            inner.talking = false;
        }
    }

    /// Voice activity: frames go out while the level is above a speaking threshold, plus 300 ms.
    fn frame(audio: &Rc<RefCell<Self>>, samples: js_sys::Float32Array) {
        let vec = samples.to_vec();
        if vec.is_empty() {
            return;
        }
        let sum: f64 = vec.iter().map(|&s| (s as f64) * (s as f64)).sum();
        let loud = (sum / vec.len() as f64).sqrt() > 0.012;

        let (encoder, to_encode): (JsValue, Vec<(f64, js_sys::Float32Array)>) = {
            let mut inner = audio.borrow_mut();
            let Some(encoder) = inner.encoder.clone() else {
                return;
            };
            if js_get(&encoder, "state").as_string().as_deref() != Some("configured") {
                return;
            }
            if inner.muted || inner.send.is_none() {
                inner.talking = false;
                inner.previous = Some(samples);
                return;
            }
            inner.hang = if loud {
                15
            } else {
                inner.hang.saturating_sub(1)
            };
            let sending = inner.hang > 0;
            let mut queue = Vec::new();
            if sending
                && !inner.talking
                && let Some(prev) = inner.previous.clone()
            {
                let ts = inner.time;
                inner.time += 20_000.0;
                queue.push((ts, prev));
            }
            if sending {
                let ts = inner.time;
                inner.time += 20_000.0;
                queue.push((ts, samples.clone()));
            }
            inner.talking = sending;
            inner.previous = Some(samples);
            (encoder, queue)
        };

        for (ts, buf) in to_encode {
            let init = js_obj(&[
                ("format", "f32".into()),
                ("sampleRate", 48000.into()),
                ("numberOfFrames", (buf.length() as f64).into()),
                ("numberOfChannels", 1.into()),
                ("timestamp", ts.into()),
                ("data", buf.into()),
            ]);
            if let Ok(data) = js_construct("AudioData", &[init]) {
                let _ = js_call(&encoder, "encode", &[data.clone()]);
                let _ = js_call(&data, "close", &[]);
            }
        }
    }

    fn play(audio: &Rc<RefCell<Self>>, user: String, opus: &[u8]) {
        let decoder = {
            let mut inner = audio.borrow_mut();
            if inner.ctx.is_none() || inner.deaf {
                return;
            }
            inner.heard.insert(user.clone(), perf_now());
            if let Some(entry) = inner.decoders.get(&user) {
                entry.decoder.clone()
            } else {
                let next = Rc::new(Cell::new(0.0f64));
                let weak_out = Rc::downgrade(audio);
                let next_out = next.clone();
                let output = Closure::<dyn FnMut(JsValue)>::new(move |data: JsValue| {
                    if let Some(a) = weak_out.upgrade() {
                        Self::schedule(&a, &next_out, data);
                    } else {
                        let _ = js_call(&data, "close", &[]);
                    }
                });
                let weak_err = Rc::downgrade(audio);
                let user_err = user.clone();
                let error = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| {
                    if let Some(a) = weak_err.upgrade()
                        && let Ok(mut guard) = a.try_borrow_mut()
                    {
                        guard.decoders.remove(&user_err);
                    }
                });
                let init = js_obj(&[
                    ("output", output.as_ref().clone()),
                    ("error", error.as_ref().clone()),
                ]);
                let Ok(decoder) = js_construct("AudioDecoder", &[init]) else {
                    return;
                };
                let cfg = js_obj(&[
                    ("codec", "opus".into()),
                    ("sampleRate", 48000.into()),
                    ("numberOfChannels", 2.into()),
                ]);
                if js_call(&decoder, "configure", &[cfg]).is_err() {
                    return;
                }
                let dec_clone = decoder.clone();
                inner.decoders.insert(
                    user,
                    DecoderEntry {
                        decoder,
                        _next: next,
                        _output: output,
                        _error: error,
                    },
                );
                dec_clone
            }
        };

        if js_get(&decoder, "state").as_string().as_deref() == Some("configured") {
            let opus_arr = js_sys::Uint8Array::from(opus);
            let init = js_obj(&[
                ("type", "key".into()),
                ("timestamp", (perf_now() * 1000.0).round().into()),
                ("data", opus_arr.into()),
            ]);
            if let Ok(chunk) = js_construct("EncodedAudioChunk", &[init]) {
                let _ = js_call(&decoder, "decode", &[chunk]);
            }
        }
    }

    fn schedule(audio: &Rc<RefCell<Self>>, next_cell: &Cell<f64>, data: JsValue) {
        let (ctx, master) = {
            let inner = audio.borrow();
            match (inner.ctx.clone(), inner.master.clone()) {
                (Some(c), Some(m)) => (c, m),
                _ => {
                    let _ = js_call(&data, "close", &[]);
                    return;
                }
            }
        };
        let channels = js_get(&data, "numberOfChannels").as_f64().unwrap_or(2.0) as u32;
        let frames = js_get(&data, "numberOfFrames").as_f64().unwrap_or(0.0) as u32;
        let rate = js_get(&data, "sampleRate").as_f64().unwrap_or(48000.0);
        if let Ok(buffer) = js_call(
            &ctx,
            "createBuffer",
            &[channels.into(), frames.into(), rate.into()],
        ) {
            for c in 0..channels {
                if let Ok(ch_data) = js_call(&buffer, "getChannelData", &[c.into()]) {
                    let opts = js_obj(&[("planeIndex", c.into()), ("format", "f32-planar".into())]);
                    let _ = js_call(&data, "copyTo", &[ch_data, opts]);
                }
            }
            let _ = js_call(&data, "close", &[]);
            let now = js_get(&ctx, "currentTime").as_f64().unwrap_or(0.0);
            let mut next = next_cell.get();
            if next < now + 0.01 || next > now + 0.4 {
                next = now + 0.06;
            }
            if let Ok(source) = js_call(&ctx, "createBufferSource", &[]) {
                js_set(&source, "buffer", &buffer);
                let _ = js_call(&source, "connect", &[master]);
                let _ = js_call(&source, "start", &[next.into()]);
                let duration = js_get(&buffer, "duration").as_f64().unwrap_or(0.02);
                next_cell.set(next + duration);
            }
        } else {
            let _ = js_call(&data, "close", &[]);
        }
    }

    fn tick(audio: &Rc<RefCell<Self>>) {
        let notify = {
            let mut inner = audio.borrow_mut();
            let now = perf_now();
            inner.heard.retain(|_, at| now - *at < 300.0);
            let mut ids: BTreeSet<String> = inner.heard.keys().cloned().collect();
            if inner.talking && !inner.me.is_empty() {
                ids.insert(inner.me.clone());
            }
            let key = ids.iter().cloned().collect::<Vec<_>>().join(",");
            if key != inner.shown {
                inner.shown = key;
                Some((inner.on_speaking.clone(), ids))
            } else {
                None
            }
        };
        if let Some((cb, ids)) = notify {
            cb(ids);
        }
    }

    fn close(audio: &Rc<RefCell<Self>>) {
        let on_speaking = {
            let mut inner = audio.borrow_mut();
            if let Some(id) = inner.timer.take() {
                window().clear_interval_with_handle(id);
            }
            inner._timer_cb = None;
            if let Some(stream) = inner.stream.take() {
                stop_stream_tracks(&stream);
            }
            if let Some(enc) = inner.encoder.take() {
                let _ = js_call(&enc, "close", &[]);
            }
            inner._encoder_output = None;
            inner._encoder_error = None;
            // The port can still deliver frames; detach the handler before its closure is dropped.
            if let Some(node) = inner._worklet_node.take() {
                js_set(&js_get(&node, "port"), "onmessage", &JsValue::NULL);
                let _ = js_call(&node, "disconnect", &[]);
            }
            inner._worklet_onmessage = None;
            for entry in inner.decoders.values() {
                let _ = js_call(&entry.decoder, "close", &[]);
            }
            inner.decoders.clear();
            if let Some(ctx) = inner.ctx.take() {
                let _ = js_call(&ctx, "close", &[]);
            }
            inner.master = None;
            inner.send = None;
            inner.on_speaking.clone()
        };
        on_speaking(BTreeSet::new());
    }
}

struct VoiceLink {
    socket: Option<web_sys::WebSocket>,
    _onopen: Option<Closure<dyn FnMut(web_sys::Event)>>,
    _onmessage: Option<Closure<dyn FnMut(web_sys::MessageEvent)>>,
    _onclose: Option<Closure<dyn FnMut(web_sys::CloseEvent)>>,
    closed: bool,
    retry: u32,
    timer: Option<i32>,
    _timer_cb: Option<Closure<dyn FnMut()>>,
    want_here: bool,
    me: String,
}

type ConnectedSinceMap = Rc<RefCell<BTreeMap<String, (String, String, f64)>>>;

struct VoiceHubInner {
    links: BTreeMap<String, VoiceLink>,
    audio: Option<Rc<RefCell<VoiceAudioInner>>>,
    audio_account: String,
    connected_since: ConnectedSinceMap,
    session: BrowserSession,
    snapshot: RwSignal<VoiceSnapshot>,
}

impl VoiceHubInner {
    fn update_account(hub: &Rc<RefCell<Self>>, id: &str, change: impl FnOnce(&mut AccountVoice)) {
        let (snap, connected_since) = {
            let inner = hub.borrow();
            (inner.snapshot, inner.connected_since.clone())
        };
        let _ = snap.try_update(|s| {
            let entry = s.accounts.entry(id.to_owned()).or_default();
            change(entry);
            let call_key = entry.call.as_ref().and_then(|c| {
                (c.state == "connected").then(|| (c.server_id.clone(), c.channel_id.clone()))
            });
            let mut cs = connected_since.borrow_mut();
            match call_key {
                Some((server_id, channel_id)) => {
                    let keep = cs
                        .get(id)
                        .is_some_and(|(srv, ch, _)| *srv == server_id && *ch == channel_id);
                    if !keep {
                        cs.insert(id.to_owned(), (server_id, channel_id, js_sys::Date::now()));
                    }
                }
                None => {
                    cs.remove(id);
                }
            }
        });
        Self::follow(hub);
    }

    fn sync(hub: &Rc<RefCell<Self>>, accounts: &[(String, String)]) {
        let to_open: Vec<String> = {
            let mut inner = hub.borrow_mut();
            let wanted: BTreeSet<&str> = accounts.iter().map(|(id, _)| id.as_str()).collect();
            let removed: Vec<String> = inner
                .links
                .keys()
                .filter(|id| !wanted.contains(id.as_str()))
                .cloned()
                .collect();
            for id in removed {
                inner.connected_since.borrow_mut().remove(&id);
                if let Some(mut link) = inner.links.remove(&id) {
                    link.closed = true;
                    if let Some(t) = link.timer.take() {
                        window().clear_timeout_with_handle(t);
                    }
                    if let Some(s) = link.socket.take() {
                        s.set_onopen(None);
                        s.set_onmessage(None);
                        s.set_onclose(None);
                        let _ = s.close();
                    }
                }
            }
            let mut open = Vec::new();
            for (id, me) in accounts {
                if let Some(existing) = inner.links.get_mut(id) {
                    if !me.is_empty() {
                        existing.me = me.clone();
                    }
                } else {
                    inner.links.insert(
                        id.clone(),
                        VoiceLink {
                            socket: None,
                            _onopen: None,
                            _onmessage: None,
                            _onclose: None,
                            closed: false,
                            retry: 0,
                            timer: None,
                            _timer_cb: None,
                            want_here: false,
                            me: me.clone(),
                        },
                    );
                    open.push(id.clone());
                }
            }
            open
        };
        for id in to_open {
            Self::open_socket(hub, &id);
        }
    }

    fn open_socket(hub: &Rc<RefCell<Self>>, id: &str) {
        if hub.borrow().links.get(id).is_none_or(|l| l.closed) {
            return;
        }
        let loc = window().location();
        let proto = if loc.protocol().unwrap_or_default() == "https:" {
            "wss"
        } else {
            "ws"
        };
        let host = loc.host().unwrap_or_default();
        let url = format!(
            "{proto}://{host}{BASE}/live?{}",
            search_params(&[("accountId", id)])
        );
        let Ok(socket) = web_sys::WebSocket::new(&url) else {
            return;
        };
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);

        let weak_open = Rc::downgrade(hub);
        let id_open = id.to_owned();
        let onopen = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            if let Some(h) = weak_open.upgrade()
                && let Some(link) = h.borrow_mut().links.get_mut(&id_open)
            {
                link.retry = 0;
            }
        });
        socket.set_onopen(Some(onopen.as_ref().unchecked_ref()));

        let weak_msg = Rc::downgrade(hub);
        let id_msg = id.to_owned();
        let onmessage = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
            move |event: web_sys::MessageEvent| {
                let Some(h) = weak_msg.upgrade() else { return };
                let data = event.data();
                if data.dyn_ref::<js_sys::ArrayBuffer>().is_some() {
                    let bytes = js_sys::Uint8Array::new(&data).to_vec();
                    if bytes.len() > 9 && bytes[0] == 1 {
                        let audio = {
                            let inner = h.borrow();
                            if inner.audio_account == id_msg {
                                inner.audio.clone()
                            } else {
                                None
                            }
                        };
                        if let Some(audio) = audio {
                            let user =
                                u64::from_be_bytes(bytes[1..9].try_into().unwrap()).to_string();
                            VoiceAudioInner::play(&audio, user, &bytes[9..]);
                        }
                    }
                    return;
                }
                if let Some(text) = data.as_string()
                    && let Ok(message) = serde_json::from_str::<ChatLiveMessage>(&text)
                {
                    Self::receive(&h, &id_msg, message);
                }
            },
        );
        socket.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

        let weak_close = Rc::downgrade(hub);
        let id_close = id.to_owned();
        let sock_close = socket.clone();
        let onclose = Closure::<dyn FnMut(web_sys::CloseEvent)>::new(move |_| {
            let Some(h) = weak_close.upgrade() else {
                return;
            };
            let retry_delay = {
                let mut inner = h.borrow_mut();
                let Some(link) = inner.links.get_mut(&id_close) else {
                    return;
                };
                let same = link.socket.as_ref().is_some_and(|s| *s == sock_close);
                if !same {
                    return;
                }
                link.socket = None;
                link._onopen = None;
                link._onmessage = None;
                link._onclose = None;
                if link.closed {
                    None
                } else {
                    let exp = link.retry.min(10);
                    link.retry += 1;
                    Some((1000u32.saturating_mul(1u32 << exp)).min(15_000))
                }
            };
            Self::update_account(&h, &id_close, |v| {
                v.online = false;
                v.here = false;
                v.elsewhere = false;
            });
            if let Some(delay) = retry_delay {
                let weak_retry = Rc::downgrade(&h);
                let id_retry = id_close.clone();
                let cb = Closure::<dyn FnMut()>::new(move || {
                    if let Some(h) = weak_retry.upgrade() {
                        Self::open_socket(&h, &id_retry);
                    }
                });
                if let Ok(timer) = window().set_timeout_with_callback_and_timeout_and_arguments_0(
                    cb.as_ref().unchecked_ref(),
                    delay as i32,
                ) && let Some(link) = h.borrow_mut().links.get_mut(&id_close)
                {
                    link.timer = Some(timer);
                    link._timer_cb = Some(cb);
                }
            }
        });
        socket.set_onclose(Some(onclose.as_ref().unchecked_ref()));

        if let Some(link) = hub.borrow_mut().links.get_mut(id) {
            link.socket = Some(socket);
            link._onopen = Some(onopen);
            link._onmessage = Some(onmessage);
            link._onclose = Some(onclose);
        }
    }

    fn receive(hub: &Rc<RefCell<Self>>, id: &str, message: ChatLiveMessage) {
        match message {
            ChatLiveMessage::Hello { servers, call } => {
                let has_call = call.is_some();
                Self::update_account(hub, id, |v| {
                    v.servers = servers;
                    v.call = call;
                    v.online = true;
                });
                let want = hub.borrow().links.get(id).is_some_and(|l| l.want_here);
                if want && has_call {
                    Self::claim(hub, id);
                }
            }
            ChatLiveMessage::Voice {
                servers: incoming,
                complete,
            } => {
                Self::update_account(hub, id, |v| {
                    if complete {
                        v.servers.clear();
                    }
                    for (server, channels) in incoming {
                        if channels.is_empty() {
                            v.servers.remove(&server);
                        } else {
                            v.servers.insert(server, channels);
                        }
                    }
                });
            }
            ChatLiveMessage::Call { call, ended } => {
                if call.is_none() {
                    if let Some(link) = hub.borrow_mut().links.get_mut(id) {
                        link.want_here = false;
                    }
                    if let Some(ended) = ended
                        && !ended.message.is_empty()
                    {
                        let snap = hub.borrow().snapshot;
                        let _ = snap.try_update(|s| {
                            s.ended = Some(EndedNotice {
                                account_id: id.to_owned(),
                                message: ended.message,
                                at: js_sys::Date::now(),
                            });
                        });
                    }
                }
                Self::update_account(hub, id, |v| {
                    v.call = call;
                });
            }
            ChatLiveMessage::Owner { here, elsewhere } => {
                if !here
                    && elsewhere
                    && let Some(link) = hub.borrow_mut().links.get_mut(id)
                {
                    link.want_here = false;
                }
                Self::update_account(hub, id, |v| {
                    v.here = here;
                    v.elsewhere = elsewhere;
                });
            }
        }
    }

    fn claim(hub: &Rc<RefCell<Self>>, id: &str) {
        let socket = hub.borrow().links.get(id).and_then(|l| l.socket.clone());
        if let Some(s) = socket
            && s.ready_state() == web_sys::WebSocket::OPEN
        {
            let _ = s.send_with_str(r#"{"type":"claim"}"#);
        }
    }

    fn follow(hub: &Rc<RefCell<Self>>) {
        let snap = hub.borrow().snapshot;
        let active = snap.try_get_untracked().and_then(|s| {
            s.accounts.into_iter().find_map(|(id, v)| {
                if v.here {
                    v.call.map(|c| (id, c))
                } else {
                    None
                }
            })
        });
        let Some((id, call)) = active else {
            let old = {
                let mut inner = hub.borrow_mut();
                inner.audio_account.clear();
                inner.audio.take()
            };
            if let Some(old) = old {
                VoiceAudioInner::close(&old);
            }
            return;
        };

        let (audio, needs_open, old) = {
            let mut inner = hub.borrow_mut();
            let same = inner.audio.is_some() && inner.audio_account == id;
            if same {
                (inner.audio.clone().unwrap(), false, None)
            } else {
                let old = inner.audio.take();
                let me = inner
                    .links
                    .get(&id)
                    .map(|l| l.me.clone())
                    .unwrap_or_default();
                let on_speaking: Rc<dyn Fn(BTreeSet<String>)> = Rc::new(move |ids| {
                    let _ = snap.try_update(|s| s.speaking = ids);
                });
                let created = VoiceAudioInner::new(me, on_speaking);
                inner.audio = Some(created.clone());
                inner.audio_account = id.clone();
                (created, true, old)
            }
        };
        if let Some(old) = old {
            VoiceAudioInner::close(&old);
        }
        if needs_open {
            let audio_open = audio.clone();
            spawn_local(async move {
                let mic = VoiceAudioInner::open(audio_open).await;
                let _ = snap.try_update(|s| s.mic = mic);
            });
        }
        let weak_hub = Rc::downgrade(hub);
        let id_send = id.clone();
        audio.borrow_mut().send = Some(Rc::new(move |frame: Vec<u8>| {
            if let Some(h) = weak_hub.upgrade() {
                let socket = h
                    .borrow()
                    .links
                    .get(&id_send)
                    .and_then(|l| l.socket.clone());
                if let Some(s) = socket
                    && s.ready_state() == web_sys::WebSocket::OPEN
                    && s.buffered_amount() < 64_000
                {
                    let _ = s.send_with_u8_array(&frame);
                }
            }
        }));
        VoiceAudioInner::set(&audio, call.self_mute, call.self_deaf);
    }

    async fn prepare(hub: Rc<RefCell<Self>>, id: String) -> MicState {
        let snap = hub.borrow().snapshot;
        let (audio, old) = {
            let mut inner = hub.borrow_mut();
            let same = inner.audio.is_some() && inner.audio_account == id;
            if same {
                (inner.audio.clone().unwrap(), None)
            } else {
                let old = inner.audio.take();
                let me = inner
                    .links
                    .get(&id)
                    .map(|l| l.me.clone())
                    .unwrap_or_default();
                let on_speaking: Rc<dyn Fn(BTreeSet<String>)> = Rc::new(move |ids| {
                    let _ = snap.try_update(|s| s.speaking = ids);
                });
                let created = VoiceAudioInner::new(me, on_speaking);
                inner.audio = Some(created.clone());
                inner.audio_account = id;
                (created, old)
            }
        };
        if let Some(old) = old {
            VoiceAudioInner::close(&old);
        }
        let mic = VoiceAudioInner::open(audio).await;
        let _ = snap.try_update(|s| s.mic = mic);
        mic
    }

    fn dispose(hub: &Rc<RefCell<Self>>) {
        let audio = {
            let mut inner = hub.borrow_mut();
            inner.connected_since.borrow_mut().clear();
            for mut link in std::mem::take(&mut inner.links).into_values() {
                link.closed = true;
                if let Some(t) = link.timer.take() {
                    window().clear_timeout_with_handle(t);
                }
                if let Some(s) = link.socket.take() {
                    s.set_onopen(None);
                    s.set_onmessage(None);
                    s.set_onclose(None);
                    let _ = s.close();
                }
            }
            inner.audio_account.clear();
            inner.audio.take()
        };
        if let Some(audio) = audio {
            VoiceAudioInner::close(&audio);
        }
    }
}

#[derive(Clone, Copy)]
pub struct VoiceHub {
    inner: StoredValue<SendWrapper<Rc<RefCell<VoiceHubInner>>>>,
    connected_since: StoredValue<SendWrapper<ConnectedSinceMap>>,
    pub snapshot: RwSignal<VoiceSnapshot>,
}

#[derive(Deserialize)]
struct CodeResponse {
    code: String,
}

impl VoiceHub {
    fn rc(self) -> Option<Rc<RefCell<VoiceHubInner>>> {
        self.inner.try_with_value(|sw| Rc::clone(&**sw))
    }

    pub(super) fn call_started_ms(self, server_id: &str, channel_id: &str) -> Option<f64> {
        self.connected_since
            .try_with_value(|cs| {
                cs.borrow().values().find_map(|(srv, ch, started)| {
                    (srv == server_id && ch == channel_id).then_some(*started)
                })
            })
            .flatten()
    }

    pub fn sync(self, accounts: &[(String, String)]) {
        if let Some(hub) = self.rc() {
            VoiceHubInner::sync(&hub, accounts);
        }
    }

    pub async fn join(
        self,
        account_id: String,
        thread_id: String,
        muted: bool,
    ) -> Result<(), String> {
        if !voice_supported() {
            return Err("This browser cannot take part in voice calls.".into());
        }
        let Some(hub) = self.rc() else { return Ok(()) };
        let others: Vec<String> = self
            .snapshot
            .try_get_untracked()
            .map(|s| {
                s.accounts
                    .into_iter()
                    .filter_map(|(id, v)| {
                        if id != account_id && v.here && v.call.is_some() {
                            Some(id)
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        for other in others {
            self.leave(other).await?;
        }
        let mic = VoiceHubInner::prepare(hub.clone(), account_id.clone()).await;
        if let Some(link) = hub.borrow_mut().links.get_mut(&account_id) {
            link.want_here = true;
        }
        let csrf = hub.borrow().session.csrf_token.clone();
        let body = json!({ "threadId": thread_id, "muted": muted || mic != MicState::On });
        if let Err(e) =
            api::send::<serde_json::Value>(&format!("{BASE}/voice/join"), "POST", Some(body), &csrf)
                .await
        {
            if let Some(link) = hub.borrow_mut().links.get_mut(&account_id) {
                link.want_here = false;
            }
            VoiceHubInner::follow(&hub);
            return Err(e.message);
        }
        VoiceHubInner::claim(&hub, &account_id);
        Ok(())
    }

    pub async fn leave(self, account_id: String) -> Result<(), String> {
        let Some(hub) = self.rc() else { return Ok(()) };
        if let Some(link) = hub.borrow_mut().links.get_mut(&account_id) {
            link.want_here = false;
        }
        let csrf = hub.borrow().session.csrf_token.clone();
        api::send::<serde_json::Value>(
            &format!("{BASE}/voice/leave"),
            "POST",
            Some(json!({ "accountId": account_id })),
            &csrf,
        )
        .await
        .map(|_| ())
        .map_err(|e| e.message)
    }

    pub async fn set_self(
        self,
        account_id: String,
        muted: bool,
        deafened: bool,
    ) -> Result<(), String> {
        let Some(hub) = self.rc() else { return Ok(()) };
        let blocked = self
            .snapshot
            .try_get_untracked()
            .is_some_and(|s| s.mic == MicState::Blocked);
        if !muted && blocked {
            let mic = VoiceHubInner::prepare(hub.clone(), account_id.clone()).await;
            if mic != MicState::On {
                return Err(
                    "Allow the microphone for this site in the browser, then try again.".into(),
                );
            }
        }
        let csrf = hub.borrow().session.csrf_token.clone();
        api::send::<serde_json::Value>(
            &format!("{BASE}/voice/set"),
            "POST",
            Some(json!({ "accountId": account_id, "muted": muted, "deafened": deafened })),
            &csrf,
        )
        .await
        .map(|_| ())
        .map_err(|e| e.message)
    }

    pub async fn use_here(self, account_id: String) -> Result<(), String> {
        let Some(hub) = self.rc() else { return Ok(()) };
        if !hub.borrow().links.contains_key(&account_id) {
            return Ok(());
        }
        VoiceHubInner::prepare(hub.clone(), account_id.clone()).await;
        if let Some(link) = hub.borrow_mut().links.get_mut(&account_id) {
            link.want_here = true;
        }
        VoiceHubInner::claim(&hub, &account_id);
        Ok(())
    }

    pub async fn code(self, account_id: String) -> Result<String, String> {
        let url = format!(
            "{BASE}/voice/code?{}",
            search_params(&[("accountId", &account_id)])
        );
        api::get::<CodeResponse>(&url)
            .await
            .map(|r| r.code)
            .map_err(|e| e.message)
    }

    pub fn dispose(self) {
        if let Some(hub) = self.rc() {
            VoiceHubInner::dispose(&hub);
        }
    }
}

/// Voice for the given Discord accounts; sockets live as long as the Chat page (`useVoice`).
pub fn use_voice(session: BrowserSession, accounts: Signal<Vec<ChatAccount>>) -> VoiceHub {
    let snapshot = RwSignal::new(VoiceSnapshot::default());
    let connected_since = Rc::new(RefCell::new(BTreeMap::new()));
    let rc = Rc::new(RefCell::new(VoiceHubInner {
        links: BTreeMap::new(),
        audio: None,
        audio_account: String::new(),
        connected_since: connected_since.clone(),
        session,
        snapshot,
    }));
    let hub = VoiceHub {
        inner: StoredValue::new(SendWrapper::new(rc)),
        connected_since: StoredValue::new(SendWrapper::new(connected_since)),
        snapshot,
    };
    provide_context(hub);
    Effect::new(move |_| {
        let list: Vec<(String, String)> = accounts
            .get()
            .into_iter()
            .filter(|a| {
                a.platform == "discord"
                    && a.state != "loggedOut"
                    && a.state != "linking"
                    && a.state != "failed"
            })
            .map(|a| (a.id, a.jid))
            .collect();
        hub.sync(&list);
    });
    on_cleanup(move || hub.dispose());
    hub
}

pub(super) fn voice_initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.is_empty() {
        return "?".into();
    }
    let mut out = String::new();
    if let Some(c) = words[0].chars().next() {
        out.push(c);
    }
    if words.len() > 1
        && let Some(c) = words[words.len() - 1].chars().next()
    {
        out.push(c);
    }
    out.to_uppercase()
}

pub(super) fn state_label(call: &ChatVoiceCall) -> &'static str {
    match call.state.as_str() {
        "connected" => "Voice connected",
        "reconnecting" => "Reconnecting\u{2026}",
        _ => "Connecting\u{2026}",
    }
}

pub(super) fn format_duration(secs: u32) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}
