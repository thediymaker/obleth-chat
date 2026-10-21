//! Talks to real object storage; run with OBLETH_CHAT_S3_* set and `--ignored`.
use obleth_chat::knowledge::ObjectStore;

#[tokio::test]
#[ignore]
async fn round_trip_against_real_storage() {
    let store = ObjectStore::from_env()
        .unwrap()
        .expect("set OBLETH_CHAT_S3_ENDPOINT");
    let key = format!("test/{} space ü.txt", uuid::Uuid::new_v4());
    store
        .put(&key, b"hello knowledge".to_vec(), "text/plain")
        .await
        .unwrap();
    assert_eq!(store.get(&key, 1024).await.unwrap(), b"hello knowledge");
    let link = store.presign_get(&key, 60, "notes \"v2\" ü.txt", "text/plain; charset=utf-8");
    let response = reqwest::get(link).await.unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let disposition = response.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        disposition.contains("filename*=UTF-8''notes%20%22v2%22%20%C3%BC.txt"),
        "{disposition}"
    );
    assert_eq!(response.text().await.unwrap(), "hello knowledge");
    store.delete(&key).await.unwrap();
    assert!(store.get(&key, 1024).await.is_err());
    store.delete(&key).await.unwrap();
}
