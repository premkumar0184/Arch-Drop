use archdrop_mesh::stream::{
    decode_stream_payload, encode_stream_payload, DerpStream, FLAG_ACK, FLAG_DATA,
    STREAM_HEADER_LEN,
};
use bytes::Bytes;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn test_stream_header_encode_decode() {
    let payload = b"Hello virtual stream packet payload";
    let encoded = encode_stream_payload(42, FLAG_DATA | FLAG_ACK, payload);

    assert_eq!(encoded.len(), STREAM_HEADER_LEN + payload.len());

    let (stream_id, flags, data) =
        decode_stream_payload(encoded).expect("Decode stream header must succeed");

    assert_eq!(stream_id, 42);
    assert_eq!(flags, FLAG_DATA | FLAG_ACK);
    assert_eq!(&data[..], payload);

    // Truncated header
    let short_bytes = Bytes::from_static(&[0x00, 0x01, 0x02]);
    assert!(decode_stream_payload(short_bytes).is_err());
}

#[tokio::test]
async fn test_stream_pair_basic_read_write() {
    let (mut stream_a, mut stream_b) = DerpStream::pair();

    let test_data = b"The quick brown fox jumps over the lazy dog";

    // Write from A
    stream_a.write_all(test_data).await.unwrap();
    stream_a.flush().await.unwrap();
    stream_a.shutdown().await.unwrap();

    // Read on B
    let mut received = Vec::new();
    stream_b.read_to_end(&mut received).await.unwrap();

    assert_eq!(received, test_data);
}

#[tokio::test]
async fn test_stream_large_streaming_transfer() {
    let (mut stream_a, mut stream_b) = DerpStream::pair();

    // 512 KiB synthetic payload
    let mut payload = vec![0u8; 512 * 1024];
    for (idx, byte) in payload.iter_mut().enumerate() {
        *byte = (idx % 251) as u8;
    }

    let mut expected_hasher = Sha256::new();
    expected_hasher.update(&payload);
    let expected_hash = expected_hasher.finalize();

    let payload_clone = payload.clone();

    // Sender task
    let send_handle = tokio::spawn(async move {
        stream_a.write_all(&payload_clone).await.unwrap();
        stream_a.flush().await.unwrap();
        stream_a.shutdown().await.unwrap();
    });

    // Receiver task
    let recv_handle = tokio::spawn(async move {
        let mut received = Vec::new();
        stream_b.read_to_end(&mut received).await.unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&received);
        hasher.finalize()
    });

    let (_, actual_hash) = tokio::join!(send_handle, recv_handle);
    let actual_hash = actual_hash.unwrap();

    assert_eq!(actual_hash, expected_hash, "SHA-256 digest of transferred stream must match");
}

#[tokio::test]
async fn test_stream_bidirectional_concurrent() {
    let (stream_a, stream_b) = DerpStream::pair();
    let (mut read_a, mut write_a) = tokio::io::split(stream_a);
    let (mut read_b, mut write_b) = tokio::io::split(stream_b);

    let data_a_to_b = vec![0x33u8; 64 * 1024];
    let data_b_to_a = vec![0x77u8; 64 * 1024];

    let data_a_to_b_clone = data_a_to_b.clone();
    let data_b_to_a_clone = data_b_to_a.clone();

    // Task 1: A sends to B, and reads from B
    let handle_a = tokio::spawn(async move {
        let write_fut = async {
            write_a.write_all(&data_a_to_b_clone).await.unwrap();
            write_a.flush().await.unwrap();
            write_a.shutdown().await.unwrap();
        };

        let mut received_at_a = Vec::new();
        let read_fut = async {
            read_a.read_to_end(&mut received_at_a).await.unwrap();
            received_at_a
        };

        let (_, rec) = tokio::join!(write_fut, read_fut);
        rec
    });

    // Task 2: B sends to A, and reads from A
    let handle_b = tokio::spawn(async move {
        let write_fut = async {
            write_b.write_all(&data_b_to_a_clone).await.unwrap();
            write_b.flush().await.unwrap();
            write_b.shutdown().await.unwrap();
        };

        let mut received_at_b = Vec::new();
        let read_fut = async {
            read_b.read_to_end(&mut received_at_b).await.unwrap();
            received_at_b
        };

        let (_, rec) = tokio::join!(write_fut, read_fut);
        rec
    });

    let (res_a, res_b) = tokio::join!(handle_a, handle_b);
    let received_at_a = res_a.unwrap();
    let received_at_b = res_b.unwrap();

    assert_eq!(received_at_b, data_a_to_b);
    assert_eq!(received_at_a, data_b_to_a);
}
