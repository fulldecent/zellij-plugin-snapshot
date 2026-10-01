use anyhow::{anyhow, Context, Result};
use prost::Message;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use wasmi::{Caller, Engine, Func, Linker, Module, Store};
use wasmi_wasi::sync::WasiCtxBuilder;
use wasmi_wasi::wasi_common::pipe::{ReadPipe, WritePipe};
use wasmi_wasi::WasiCtx;
use zellij_utils::data::{
    Event, GetSessionListResponse, PluginCommand, PluginIds, SessionListSnapshot,
};
use zellij_utils::input::layout::PluginUserConfiguration;
use zellij_utils::plugin_api::action::ProtobufPluginConfiguration;
use zellij_utils::plugin_api::event::ProtobufEvent;
use zellij_utils::plugin_api::plugin_command::{
    ProtobufGenerateRandomNameResponse, ProtobufGetLayoutDirResponse, ProtobufGetSessionListResponse,
    ProtobufPluginCommand,
};
use zellij_utils::plugin_api::plugin_ids::{ProtobufPluginIds, ProtobufZellijVersion};

use crate::script::{HostIds, Step};

pub struct PluginHost {
    store: Store<Env>,
    instance: wasmi::Instance,
}

pub struct Env {
    wasi: WasiCtx,
    stdin: Arc<Mutex<VecDeque<u8>>>,
    stdout: Arc<Mutex<VecDeque<u8>>>,
    ids: HostIds,
    session_list: SessionListSnapshot,
    pub effects: Vec<String>,
    pub subscriptions: HashSet<String>,
    pub selectable: Option<bool>,
}

impl PluginHost {
    pub fn load(
        wasm_path: &Path,
        config: &BTreeMap<String, String>,
        ids: &HostIds,
        session_list: SessionListSnapshot,
    ) -> Result<Self> {
        let wasm = std::fs::read(wasm_path)
            .with_context(|| format!("read wasm {}", wasm_path.display()))?;
        let engine = Engine::default();
        let module = Module::new(&engine, &wasm).context("parse wasm")?;

        let stdin = Arc::new(Mutex::new(VecDeque::new()));
        let stdout = Arc::new(Mutex::new(VecDeque::new()));

        let mut builder = WasiCtxBuilder::new();
        let _ = builder.inherit_env();
        let wasi = builder.build();
        wasi.set_stdin(Box::new(ReadPipe::new(DequeRead(stdin.clone()))));
        wasi.set_stdout(Box::new(WritePipe::new(DequeWrite(stdout.clone()))));
        wasi.set_stderr(Box::new(WritePipe::new(std::io::sink())));

        let env = Env {
            wasi,
            stdin: stdin.clone(),
            stdout: stdout.clone(),
            ids: ids.clone(),
            session_list,
            effects: Vec::new(),
            subscriptions: HashSet::new(),
            selectable: None,
        };
        let mut store = Store::new(&engine, env);
        let mut linker = Linker::new(&engine);
        wasmi_wasi::add_to_linker(&mut linker, |env: &mut Env| &mut env.wasi)
            .context("wasi linker")?;

        let host_fn = Func::wrap(&mut store, host_run_plugin_command);
        linker
            .define("zellij", "host_run_plugin_command", host_fn)
            .context("define host_run_plugin_command")?;

        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .context("instantiate")?;

        if let Some(start) = instance.get_func(&mut store, "_start") {
            if let Ok(typed) = start.typed::<(), ()>(&store) {
                typed.call(&mut store, ()).context("_start")?;
            }
        }

        let proto_cfg: ProtobufPluginConfiguration =
            PluginUserConfiguration::new(config.clone())
                .try_into()
                .map_err(|e| anyhow!("{e}"))?;
        write_object(&store.data().stdin, &proto_cfg.encode_to_vec())?;

        let load = instance
            .get_typed_func::<(), ()>(&mut store, "load")
            .context("export load")?;
        if let Err(e) = load.call(&mut store, ()) {
            let fx = store.data().effects.join("; ");
            anyhow::bail!("call load ({fx}): {e}");
        }

        Ok(Self { store, instance })
    }

    pub fn drive(&mut self, steps: &[Step]) -> Result<Vec<String>> {
        for step in steps {
            if let Some(event) = step.to_event()? {
                self.push_event(&event)?;
            }
        }
        Ok(self.store.data().effects.clone())
    }

    pub fn push_event(&mut self, event: &Event) -> Result<bool> {
        let proto: ProtobufEvent = event
            .clone()
            .try_into()
            .map_err(|e| anyhow!("event protobuf: {e}"))?;
        write_object(&self.store.data().stdin, &proto.encode_to_vec())?;
        let update = self
            .instance
            .get_typed_func::<(), i32>(&mut self.store, "update")
            .context("export update")?;
        let should = update.call(&mut self.store, ()).context("call update")?;
        Ok(should == 1)
    }

    pub fn render(&mut self, rows: u32, cols: u32) -> Result<String> {
        drain_stdout(&self.store.data().stdout);
        let render = self
            .instance
            .get_typed_func::<(i32, i32), ()>(&mut self.store, "render")
            .context("export render")?;
        render
            .call(&mut self.store, (rows as i32, cols as i32))
            .context("call render")?;
        Ok(read_stdout_string(&self.store.data().stdout))
    }
}

fn host_run_plugin_command(mut caller: Caller<'_, Env>) {
    let stdout = caller.data().stdout.clone();
    let stdin = caller.data().stdin.clone();
    let bytes = match read_bytes_json(&stdout) {
        Ok(b) => b,
        Err(e) => {
            caller.data_mut().effects.push(format!("bad host command json: {e}"));
            return;
        }
    };
    let proto = match ProtobufPluginCommand::decode(bytes.as_slice()) {
        Ok(p) => p,
        Err(e) => {
            caller.data_mut().effects.push(format!("bad command protobuf: {e}"));
            return;
        }
    };
    let command: PluginCommand = match proto.try_into() {
        Ok(c) => c,
        Err(e) => {
            caller.data_mut().effects.push(format!("command convert: {e}"));
            return;
        }
    };
    dispatch(caller.data_mut(), &stdin, command);
}

fn dispatch(env: &mut Env, stdin: &Arc<Mutex<VecDeque<u8>>>, command: PluginCommand) {
    match command {
        PluginCommand::Subscribe(events) => {
            for e in events {
                env.subscriptions.insert(format!("{e:?}"));
            }
            env.effects.push(format!("Subscribe {:?}", env.subscriptions));
        }
        PluginCommand::Unsubscribe(events) => {
            env.effects.push(format!("Unsubscribe {events:?}"));
        }
        PluginCommand::SetSelectable(s) => {
            env.selectable = Some(s);
            env.effects.push(format!("SetSelectable {s}"));
        }
        PluginCommand::ShowCursor(pos) => {
            env.effects.push(format!("ShowCursor {pos:?}"));
        }
        PluginCommand::RequestPluginPermissions(p) => {
            env.effects.push(format!("RequestPluginPermissions {p:?}"));
        }
        PluginCommand::GetPluginIds => {
            let ids = PluginIds {
                plugin_id: env.ids.plugin_id,
                zellij_pid: env.ids.zellij_pid,
                initial_cwd: env.ids.initial_cwd.clone(),
                client_id: env.ids.client_id,
            };
            let proto: ProtobufPluginIds = ids.try_into().expect("plugin ids");
            let _ = write_object(stdin, &proto.encode_to_vec());
            env.effects.push("GetPluginIds".into());
        }
        PluginCommand::GetZellijVersion => {
            let proto = ProtobufZellijVersion {
                version: "0.45.1".into(),
            };
            let _ = write_object(stdin, &proto.encode_to_vec());
            env.effects.push("GetZellijVersion".into());
        }
        PluginCommand::GenerateRandomName => {
            let proto = ProtobufGenerateRandomNameResponse {
                name: "ShotSession".into(),
            };
            let _ = write_object(stdin, &proto.encode_to_vec());
            env.effects.push("GenerateRandomName".into());
        }
        PluginCommand::GetLayoutDir => {
            let proto = ProtobufGetLayoutDirResponse {
                layout_dir: "/tmp/layouts".into(),
            };
            let _ = write_object(stdin, &proto.encode_to_vec());
            env.effects.push("GetLayoutDir".into());
        }
        PluginCommand::GetSessionList => {
            let proto: ProtobufGetSessionListResponse =
                GetSessionListResponse::Ok(env.session_list.clone()).into();
            let _ = write_object(stdin, &proto.encode_to_vec());
            env.effects.push("GetSessionList".into());
        }
        other => {
            env.effects.push(format!("{other:?}"));
        }
    }
}

fn write_object(pipe: &Arc<Mutex<VecDeque<u8>>>, object: &impl serde::Serialize) -> Result<()> {
    let json = serde_json::to_string(object)?;
    let mut g = pipe.lock().unwrap();
    writeln!(g, "{json}\r")?;
    Ok(())
}

fn read_bytes_json(pipe: &Arc<Mutex<VecDeque<u8>>>) -> Result<Vec<u8>> {
    let s = read_stdout_string(pipe).replace('\n', "\n\r");
    serde_json::from_str(&s).with_context(|| format!("stdout json: {s:?}"))
}

fn read_stdout_string(pipe: &Arc<Mutex<VecDeque<u8>>>) -> String {
    let mut g = pipe.lock().unwrap();
    let bytes: Vec<u8> = g.drain(..).collect();
    String::from_utf8_lossy(&bytes).replace('\n', "\n\r")
}

fn drain_stdout(pipe: &Arc<Mutex<VecDeque<u8>>>) {
    pipe.lock().unwrap().clear();
}

struct DequeRead(Arc<Mutex<VecDeque<u8>>>);
impl Read for DequeRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut q = self.0.lock().unwrap();
        let n = buf.len().min(q.len());
        for (i, b) in q.drain(..n).enumerate() {
            buf[i] = b;
        }
        Ok(n)
    }
}

struct DequeWrite(Arc<Mutex<VecDeque<u8>>>);
impl Write for DequeWrite {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend(buf.iter().copied());
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
