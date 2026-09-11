#![cfg(unix)]
#![forbid(unsafe_code)]

mod support;

use host_runtime::{RequestOptions, TargetKind};
use serde_json::Value;
use sha2::{Digest, Sha256};
use support::direct_host::{FixtureProcess, wait_for_store};

#[tokio::test]
#[ignore = "requires generated TS corpus; run: bun run --cwd packages/e2e-tests scripts/verify-serialized-transform-pages.ts"]
async fn serialized_transform_corpus_preserves_host_admission_and_completion() {
    let corpus_path = std::env::var_os("EIDNARA_SERIALIZED_TRANSFORM_CORPUS").expect(
        "run: bun run --cwd packages/e2e-tests scripts/verify-serialized-transform-pages.ts",
    );
    let corpus: Value =
        serde_json::from_slice(&std::fs::read(corpus_path).unwrap()).expect("pager corpus JSON");
    let session = corpus["session"].as_str().unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 19);
    let longest_float = serde_json::to_string(&-2.2250738585072014e-308_f64).unwrap();
    assert_eq!(longest_float, "-2.2250738585072014e-308");
    assert_eq!(longest_float.len(), 24);
    let fixture = FixtureProcess::start();
    let client = fixture.client().await;
    let mut complete = 0;
    let mut staged = 0;
    let mut refused = 0;
    let mut pager_refused = 0;
    for case in cases {
        let name = case["name"].as_str().unwrap();
        if case["pagerRefused"] == true {
            pager_refused += 1;
            let original = case["originalText"].as_str().unwrap();
            let parsed: Value = serde_json::from_str(original).unwrap();
            let reserialized = serde_json::to_vec(&parsed).unwrap().len();
            assert_eq!(name, "scalar-over");
            assert_eq!(reserialized, 524_289, "{name}");
            println!(
                "{name}: pager refused wire={} serde={reserialized}",
                original.len()
            );
            continue;
        }
        let route = fixture
            .open_route(&client, "context", TargetKind::ToolProvider, session)
            .await;
        wait_for_store(&client, route, session).await;
        let pages = case["pages"].as_array().unwrap();
        assert!(!pages.is_empty(), "{name}");
        if let Some(expected) = case["pageCount"].as_u64() {
            assert_eq!(pages.len() as u64, expected, "{name}: page count");
        }
        for (page, expected) in [
            (&pages[0], &case["firstPageBytes"]),
            (pages.last().unwrap(), &case["lastPageBytes"]),
        ] {
            if expected.is_number() {
                assert_eq!(&page["bytes"], expected, "{name}: boundary length");
            }
        }
        for (index, page) in pages.iter().enumerate() {
            let text = page["text"].as_str().unwrap();
            let bytes = text.as_bytes();
            assert_eq!(
                bytes.len() as u64,
                page["bytes"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(
                format!("{:x}", Sha256::digest(bytes)),
                page["sha256"].as_str().unwrap(),
                "{name}"
            );
            let parsed = serde_json::from_slice::<Value>(bytes);
            let reserialized = parsed
                .as_ref()
                .ok()
                .map(|value| serde_json::to_vec(value).unwrap().len());
            let response = client
                .request(route, bytes.to_vec(), RequestOptions::default())
                .await;
            if parsed.is_err() {
                assert_eq!(
                    case["hostRefuses"], true,
                    "{name}: unexpected parse failure"
                );
                let error = response.expect_err("lone surrogate must retain host refusal");
                assert_eq!(error.code(), "host.unrecognized_request_shape", "{name}");
                refused += 1;
                println!("{name}: wire={} refused={}", bytes.len(), error.code());
                break;
            }
            let reserialized = reserialized.expect("valid corpus page must parse");
            let parsed = parsed.unwrap();
            if parsed.get("transform_page_index").is_some() {
                assert!(
                    reserialized <= 524_288,
                    "{name}: Rust page size {reserialized}"
                );
                assert_eq!(parsed["transform_page_index"], index);
                assert_eq!(parsed["transform_page_total"], pages.len());
                assert_eq!(parsed["transform_page_complete"], index + 1 == pages.len());
            } else {
                assert_eq!(pages.len(), 1, "{name}: unpaged request");
                assert!(bytes.len() <= 32 * 1024 * 1024);
            }
            let response = response.unwrap_or_else(|error| panic!("{name} page {index}: {error}"));
            let result: Value = serde_json::from_slice(&response.body).unwrap();
            if index + 1 < pages.len() {
                assert_eq!(result["staged"], true, "{name}: {result}");
                assert_eq!(result["next_expected_index"], index + 1, "{name}");
                staged += 1;
            } else {
                assert_ne!(
                    case["hostRefuses"], true,
                    "{name}: expected a parse refusal"
                );
                assert_eq!(result["status"], "ok", "{name}: {result}");
                assert!(
                    result["messages"].is_array(),
                    "{name}: final transform response"
                );
                let control = client
                    .request(
                        route,
                        case["originalText"].as_str().unwrap().as_bytes().to_vec(),
                        RequestOptions::default(),
                    )
                    .await
                    .expect("unpaged control completes");
                let control: Value = serde_json::from_slice(&control.body).unwrap();
                assert_eq!(
                    serde_json::to_vec(&result["messages"]).unwrap(),
                    serde_json::to_vec(&control["messages"]).unwrap(),
                    "{name}: paged and unpaged served-message bytes differ"
                );
                complete += 1;
            }
            println!(
                "{name} page {index}: wire={} serde={reserialized} final={}",
                bytes.len(),
                index + 1 == pages.len()
            );
        }
        client.close_route(route).await.unwrap();
    }
    assert_eq!((complete, refused, pager_refused), (12, 6, 1));
    assert_eq!(staged, 9);
    println!(
        "complete={complete} staged={staged} host_refused={refused} pager_refused={pager_refused}"
    );
    client.close().await.unwrap();
    fixture.shutdown();
}
