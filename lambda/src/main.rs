use lambda_http::{run, service_fn, Body, Error, Request, RequestPayloadExt, Response};
use log3_lib;
use log3_lib::models::Log3Json;
use serde_json::json;

/// This is the main body for the function.
/// Write your code inside it.
/// There are some code example in the following URLs:
/// - https://github.com/awslabs/aws-lambda-rust-runtime/tree/main/examples
async fn function_handler(event: Request) -> Result<Response<Body>, Error> {
    // Extract some useful information from the request
    let log3_json: Log3Json = event.payload().expect("body error").expect("body error2");

    let run_rs = log3_lib::run(
        log3_json.chainid,
        log3_json.etherscan_api_key,
        log3_json.contract_address,
        log3_json.tx_hash,
        log3_json.endpoint,
        log3_json.method.unwrap_or_default(),
    )
    .await?;

    // Return something that implements IntoResponse.
    // It will be serialized to the right response event automatically by the runtime
    let resp = Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(json!(run_rs).to_string().into())
        .map_err(Box::new)?;
    Ok(resp)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        // disable printing the name of the module in every log line.
        .with_target(false)
        // disabling time is handy because CloudWatch will add the ingestion time.
        .without_time()
        .init();

    run(service_fn(function_handler)).await
}
