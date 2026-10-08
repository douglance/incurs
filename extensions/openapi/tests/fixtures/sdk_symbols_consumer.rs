use sdk::*;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

struct InspectingTransport;

impl Transport for InspectingTransport {
    type Error = String;

    fn call(
        &self,
        request: OperationRequest,
    ) -> impl Future<Output = Result<OperationResponse, Self::Error>> {
        async move {
            let arguments = request
                .arguments_json()
                .map_err(|error| format!("{error:?}"))?;
            match request.operation_id {
                "symbol-proof/createThing" => {
                    assert!(arguments.contains("\"path\":{\"id\":\"abc\"}"));
                    assert!(arguments.contains("\"query\":{\"type\":7}"));
                    assert!(arguments.contains("\"media_type\":\"application/json\""));
                    assert!(arguments.contains("\"origin\":{\"ok\":true}"));
                    assert!(arguments.contains("\"ref\":\"manual\""));
                    assert!(arguments.contains("\"ref-default\":\"from-ref-default\""));
                    assert!(arguments.contains("\"type_\":3"));
                }
                other => panic!("unexpected operation {other}"),
            }
            Ok(OperationResponse {
                status: 200,
                headers: Vec::new(),
                body: Field::Missing,
            })
        }
    }
}

fn main() {
    let body = CreateThingBody {
        helper: JsonValue2 {
            value: "helper".to_string(),
        },
        id: "body-id".to_string(),
        origin: LoadBalancingOriginHealthy3 { ok: true },
        ref_: Field::Value("manual".to_string()),
        ref_default: CreateThingBody::ref_default_default(),
        type_: CreateThingBody::type_default(),
        type_default: CreateThingBody::type_default_default(),
        type__2: CreateThingBody::type_2_default(),
    };
    let client = Client::new(InspectingTransport);
    let args = CreateThingArgs2 {
        id: "abc".to_string(),
        type_: CreateThingArgs2::type_default(),
        body: CreateThingBody2::ApplicationJson(Field::Value(body)),
    };
    let response = block_on(client.create_thing(args)).unwrap();
    assert_eq!(response.status(), 200);

    let forbidden = CreateThingArgs2 {
        id: "abc".to_string(),
        type_: Field::Missing,
        body: CreateThingBody2::ApplicationXNever(Field::Null),
    }
    .into_request();
    assert_eq!(
        forbidden.media_type,
        Field::Value("application/x-never".to_string())
    );
    assert_eq!(forbidden.body, Field::Value(JsonValue::Invalid));

    let omitted = FalseBodyArgs {
        body: Field::Missing,
    }
    .into_request();
    assert_eq!(omitted.media_type, Field::Missing);
    assert_eq!(omitted.body, Field::Missing);
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(raw_waker()) };
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match Future::poll(Pin::as_mut(&mut future), &mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn raw_waker() -> RawWaker {
    unsafe fn clone(_: *const ()) -> RawWaker {
        raw_waker()
    }
    unsafe fn wake(_: *const ()) {}
    unsafe fn wake_by_ref(_: *const ()) {}
    unsafe fn drop(_: *const ()) {}
    RawWaker::new(
        std::ptr::null(),
        &RawWakerVTable::new(clone, wake, wake_by_ref, drop),
    )
}
