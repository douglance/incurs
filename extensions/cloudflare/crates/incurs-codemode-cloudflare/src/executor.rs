use std::sync::Arc;

use incurs_codemode::{Clock, CodeExecutor, ConnectorDescription, ExecuteResult, ExecutionHost};
use js_sys::{Array, Object, Promise, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use worker::Stub;

use crate::{DynamicWorkerOptions, build_executor_module};

#[wasm_bindgen]
extern "C" {
    /// Cloudflare WorkerLoader binding.
    #[wasm_bindgen(typescript_type = "WorkerLoader")]
    #[derive(Clone)]
    pub type WorkerLoader;

    #[wasm_bindgen(method, js_name = load)]
    fn load(this: &WorkerLoader, settings: &JsValue) -> LoadedWorker;

    #[wasm_bindgen(typescript_type = "WorkerStub")]
    type LoadedWorker;

    #[wasm_bindgen(method, js_name = getEntrypoint)]
    fn get_entrypoint(this: &LoadedWorker) -> JsValue;
}

#[wasm_bindgen(inline_js = r#"
export async function callIncurEntrypoint(entrypoint, worker) {
  try {
    return await entrypoint.evaluate();
  } finally {
    try {
      entrypoint?.[Symbol.dispose]?.();
      worker?.[Symbol.dispose]?.();
    } catch {
      // Disposal must not mask an execution result.
    }
  }
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = callIncurEntrypoint)]
    fn call_entrypoint(entrypoint: &JsValue, worker: &LoadedWorker) -> Promise;
}

impl worker::EnvBinding for WorkerLoader {
    const TYPE_NAME: &'static str = "WorkerLoader";
}

/// Executes model-generated JavaScript inside an isolated Dynamic Worker.
pub struct DynamicWorkerExecutor {
    loader: WorkerLoader,
    dispatcher: JsValue,
    options: DynamicWorkerOptions,
}

impl DynamicWorkerExecutor {
    /// Creates an executor whose child calls return through a Durable Object.
    pub fn new(loader: WorkerLoader, dispatcher: Stub, options: DynamicWorkerOptions) -> Self {
        Self {
            loader,
            dispatcher: dispatcher.into_rpc(),
            options,
        }
    }

    async fn execute_worker(
        &self,
        code: &str,
        connectors: &[ConnectorDescription],
        execution_id: &str,
    ) -> Result<ExecuteResult, String> {
        let source = build_executor_module(code, connectors, self.options.timeout_ms)?;
        let settings = Object::new();
        Reflect::set(
            &settings,
            &"compatibilityDate".into(),
            &self.options.compatibility_date.clone().into(),
        )
        .map_err(js_error)?;
        Reflect::set(
            &settings,
            &"compatibilityFlags".into(),
            &Array::of1(&"nodejs_compat".into()),
        )
        .map_err(js_error)?;
        Reflect::set(&settings, &"mainModule".into(), &"executor.js".into()).map_err(js_error)?;
        let modules = Object::new();
        Reflect::set(&modules, &"executor.js".into(), &source.into()).map_err(js_error)?;
        for (name, source) in &self.options.modules {
            if name != "executor.js" {
                Reflect::set(&modules, &name.clone().into(), &source.clone().into())
                    .map_err(js_error)?;
            }
        }
        Reflect::set(&settings, &"modules".into(), &modules).map_err(js_error)?;
        Reflect::set(&settings, &"globalOutbound".into(), &self.dispatcher).map_err(js_error)?;
        let env = Object::new();
        Reflect::set(
            &env,
            &"__INCURS_CODEMODE_EXECUTION_ID".into(),
            &execution_id.into(),
        )
        .map_err(js_error)?;
        Reflect::set(&settings, &"env".into(), &env).map_err(js_error)?;

        let worker = self.loader.load(&settings);
        let entrypoint = worker.get_entrypoint();
        let response = JsFuture::from(call_entrypoint(&entrypoint, &worker))
            .await
            .map_err(js_error);
        let response = response?;
        serde_wasm_bindgen::from_value(response).map_err(|error| error.to_string())
    }
}

#[async_trait::async_trait(?Send)]
impl CodeExecutor for DynamicWorkerExecutor {
    async fn execute(
        &self,
        code: &str,
        connectors: &[ConnectorDescription],
        execution_id: &str,
        _host: Arc<ExecutionHost>,
    ) -> Result<ExecuteResult, String> {
        self.execute_worker(code, connectors, execution_id).await
    }
}

/// Cloudflare Workers clock for the generic Code Mode lifecycle.
#[derive(Debug, Clone, Copy, Default)]
pub struct CloudflareClock;

impl Clock for CloudflareClock {
    fn now_ms(&self) -> u64 {
        js_sys::Date::now() as u64
    }
}

fn js_error(error: JsValue) -> String {
    error
        .as_string()
        .or_else(|| {
            Reflect::get(&error, &"stack".into())
                .ok()
                .and_then(|value| value.as_string())
        })
        .or_else(|| {
            Reflect::get(&error, &"message".into())
                .ok()
                .and_then(|value| value.as_string())
        })
        .unwrap_or_else(|| format!("{error:?}"))
}
