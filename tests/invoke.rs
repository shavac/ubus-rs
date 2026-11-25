use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::mem::size_of;
use std::os::unix::net::UnixStream;
use ubus::*;

#[derive(Debug, Deserialize, PartialEq)]
struct SystemMemory {
    total: u64,
    free: u64,
    available: u64,
}

#[derive(Debug, Deserialize, PartialEq)]
struct SystemInfo {
    localtime: u32,
    uptime: u32,
    load: [u32; 3],
    memory: SystemMemory,
}

#[test]
fn test() {
    // Verify Connection stack size stays small (<= 80 bytes instead of 64 KiB)
    assert!(size_of::<Connection<UnixStream>>() <= 80);

    let (client, mut server) = UnixStream::pair().unwrap();

    std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();
        let mut command = [0u8; TEST_TX.len()];
        server.read_exact(&mut command).unwrap();
        assert_eq!(&command[..], TEST_TX);
        for i in TEST_RX {
            server.write_all(i).unwrap();
        }
    });

    let mut connection = Connection::new(client).unwrap();

    connection
        .invoke(0x13333337, "info", &[], |x| {
            #[cfg(feature = "json")]
            {
                let json = serde_json::to_string(&x.as_table()).unwrap();
                let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed["localtime"], 1600016748);
                assert_eq!(parsed["uptime"], 923);
                assert_eq!(parsed["memory"]["total"], 513433600);
            }

            let info: SystemInfo = x.deserialize().unwrap();
            assert_eq!(
                info,
                SystemInfo {
                    localtime: 1600016748,
                    uptime: 923,
                    load: [0, 0, 0],
                    memory: SystemMemory {
                        total: 513433600,
                        free: 475082752,
                        available: 454533120,
                    },
                }
            );
        })
        .unwrap();
}

#[test]
fn test_connection_invoke_typed() {
    let (client, mut server) = UnixStream::pair().unwrap();

    std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();
        let mut command = [0u8; TEST_TX.len()];
        server.read_exact(&mut command).unwrap();
        assert_eq!(&command[..], TEST_TX);
        for i in TEST_RX {
            server.write_all(i).unwrap();
        }
    });

    let mut connection = Connection::new(client).unwrap();
    let info: SystemInfo = connection.invoke_typed(0x13333337, "info", &[]).unwrap();
    assert_eq!(
        info,
        SystemInfo {
            localtime: 1600016748,
            uptime: 923,
            load: [0, 0, 0],
            memory: SystemMemory {
                total: 513433600,
                free: 475082752,
                available: 454533120,
            },
        }
    );
}

#[cfg(feature = "json")]
#[test]
fn test_args_from_json_and_bool_support() {
    let mut policy = HashMap::new();
    policy.insert("name", BlobMsgType::STRING);
    policy.insert("v6", BlobMsgType::BOOL);
    policy.insert("count", BlobMsgType::INT32);

    let mut methods = HashMap::new();
    methods.insert(
        "status",
        Method {
            name: "status",
            policy,
        },
    );

    let obj = UbusObject {
        path: "network.device",
        id: 1,
        ty: 2,
        methods,
    };

    let bytes = obj
        .args_from_json("status", r#"{"name":"eth0","v6":true,"ignored":123}"#)
        .unwrap();
    let msgs: Vec<BlobMsg> = BlobIter::<BlobMsg>::new(&bytes).collect();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].name, "name");
    assert!(matches!(msgs[0].data, BlobMsgPayload::String("eth0")));
    assert_eq!(msgs[1].name, "v6");
    assert!(matches!(msgs[1].data, BlobMsgPayload::Int8(1)));
}

#[test]
fn test_serde_blob_json_bool_and_escaping() {
    #[derive(Debug, Deserialize, PartialEq)]
    struct Nested<'a> {
        items: Vec<i32>,
        tag: &'a str,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct PayloadView<'a> {
        enabled: bool,
        disabled: bool,
        flag_u8: u8,
        flag_i8: i8,
        text: &'a str,
        count: i32,
        nested: Nested<'a>,
    }

    let mut nested_table = HashMap::new();
    nested_table.insert(
        "items",
        BlobMsgPayload::Array(vec![
            BlobMsg {
                name: "",
                data: BlobMsgPayload::Int32(10),
            },
            BlobMsg {
                name: "",
                data: BlobMsgPayload::Int32(-5),
            },
        ]),
    );
    nested_table.insert("tag", BlobMsgPayload::String("v1"));

    let mut bytes = Vec::new();
    let items = [
        BlobMsg {
            name: "enabled",
            data: BlobMsgPayload::Bool(1),
        },
        BlobMsg {
            name: "disabled",
            data: BlobMsgPayload::Bool(0),
        },
        BlobMsg {
            name: "flag_u8",
            data: BlobMsgPayload::Bool(1),
        },
        BlobMsg {
            name: "flag_i8",
            data: BlobMsgPayload::Bool(1),
        },
        BlobMsg {
            name: "text",
            data: BlobMsgPayload::String("quote \" \\ \n line"),
        },
        BlobMsg {
            name: "count",
            data: BlobMsgPayload::Int32(42),
        },
        BlobMsg {
            name: "nested",
            data: BlobMsgPayload::Table(nested_table),
        },
    ];
    for item in &items {
        BlobMsgBuilder::write_blobmsg_into(&mut bytes, item).unwrap();
    }

    #[cfg(feature = "json")]
    {
        let json = serde_json::to_string(&BlobTable(&bytes)).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["enabled"], serde_json::json!(true));
        assert_eq!(parsed["disabled"], serde_json::json!(false));
        assert_eq!(parsed["text"], serde_json::json!("quote \" \\ \n line"));
        assert_eq!(parsed["count"], serde_json::json!(42));
        assert_eq!(parsed["nested"]["items"], serde_json::json!([10, -5]));
        assert_eq!(parsed["nested"]["tag"], serde_json::json!("v1"));
    }

    let view: PayloadView<'_> = from_blob(&bytes).unwrap();
    assert_eq!(
        view,
        PayloadView {
            enabled: true,
            disabled: false,
            flag_u8: 1,
            flag_i8: 1,
            text: "quote \" \\ \n line",
            count: 42,
            nested: Nested {
                items: vec![10, -5],
                tag: "v1",
            },
        }
    );
}

#[cfg(feature = "json")]
#[test]
fn test_connection_call_and_call_typed() {
    fn read_msg(stream: &mut UnixStream) -> (UbusMsgHeader, Vec<u8>) {
        let mut prefix = [0u8; UbusMsgHeader::SIZE + BlobTag::SIZE];
        stream.read_exact(&mut prefix).unwrap();
        let header = UbusMsgHeader::from_bytes(prefix[..UbusMsgHeader::SIZE].try_into().unwrap());
        let tag = BlobTag::from_bytes(prefix[UbusMsgHeader::SIZE..].try_into().unwrap());
        let mut body = vec![0u8; tag.inner_len()];
        stream.read_exact(&mut body).unwrap();
        (header, body)
    }

    fn write_msg(stream: &mut UnixStream, header: UbusMsgHeader, payload_attrs: &[u8]) {
        let total = BlobTag::SIZE + payload_attrs.len();
        let tag = BlobTag::new(0, total, false).unwrap();
        stream.write_all(&header.to_bytes()).unwrap();
        stream.write_all(&tag.to_bytes()).unwrap();
        stream.write_all(payload_attrs).unwrap();
    }

    let (client, mut server) = UnixStream::pair().unwrap();

    let server_handle = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();

        let mut arg_policy = HashMap::new();
        arg_policy.insert("verbose", BlobMsgPayload::Int32(BlobMsgType::BOOL.value() as i32));
        let mut sig_bytes = Vec::new();
        BlobMsgBuilder::write_blobmsg_into(
            &mut sig_bytes,
            &BlobMsg {
                name: "info",
                data: BlobMsgPayload::Table(arg_policy),
            },
        )
        .unwrap();

        for _ in 0..3 {
            // 1. Handle LOOKUP
            let (lookup_hdr, _) = read_msg(&mut server);
            assert_eq!(lookup_hdr.cmd_type, UbusCmdType::LOOKUP);

            let mut attrs_buf = [0u8; 256];
            let mut bb = BlobBuilder::from_bytes(&mut attrs_buf);
            bb.push_str(BlobAttrId::OBJPATH.value(), "system").unwrap();
            bb.push_u32(BlobAttrId::OBJID.value(), 0x13333337).unwrap();
            bb.push_u32(BlobAttrId::OBJTYPE.value(), 0x11223344).unwrap();
            bb.push_slice(BlobAttrId::SIGNATURE.value(), &sig_bytes)
                .unwrap();
            let attrs_len = bb.len();

            write_msg(
                &mut server,
                UbusMsgHeader {
                    version: UbusMsgVersion::CURRENT,
                    cmd_type: UbusCmdType::DATA,
                    sequence: lookup_hdr.sequence,
                    peer: 0.into(),
                },
                &attrs_buf[..attrs_len],
            );

            let mut status_buf = [0u8; 16];
            let mut sb = BlobBuilder::from_bytes(&mut status_buf);
            sb.push_u32(BlobAttrId::STATUS.value(), 0).unwrap();
            let status_len = sb.len();
            write_msg(
                &mut server,
                UbusMsgHeader {
                    version: UbusMsgVersion::CURRENT,
                    cmd_type: UbusCmdType::STATUS,
                    sequence: lookup_hdr.sequence,
                    peer: 0.into(),
                },
                &status_buf[..status_len],
            );

            // 2. Handle INVOKE
            let (invoke_hdr, invoke_body) = read_msg(&mut server);
            assert_eq!(invoke_hdr.cmd_type, UbusCmdType::INVOKE);
            let mut saw_verbose = false;
            for attr in BlobIter::<UbusMsgAttr>::new(&invoke_body) {
                if let UbusMsgAttr::Data(data) = attr {
                    let args: HashMap<&str, bool> = from_blob(data).unwrap();
                    if args.get("verbose") == Some(&true) {
                        saw_verbose = true;
                    }
                }
            }
            assert!(saw_verbose);

            // Send TEST_RX data & status with matching sequence
            write_msg(
                &mut server,
                UbusMsgHeader {
                    version: UbusMsgVersion::CURRENT,
                    cmd_type: UbusCmdType::DATA,
                    sequence: invoke_hdr.sequence,
                    peer: 0x13333377.into(),
                },
                TEST_RX[1],
            );
            write_msg(
                &mut server,
                UbusMsgHeader {
                    version: UbusMsgVersion::CURRENT,
                    cmd_type: UbusCmdType::STATUS,
                    sequence: invoke_hdr.sequence,
                    peer: 0x13333377.into(),
                },
                TEST_RX[3],
            );
        }
    });

    let mut connection = Connection::new(client).unwrap();

    let compact = connection.call("system", "info", r#"{"verbose":true}"#).unwrap();
    let compact_val: serde_json::Value = serde_json::from_str(&compact).unwrap();
    assert_eq!(compact_val["uptime"], 923);

    let pretty = connection
        .call_pretty("system", "info", r#"{"verbose":true}"#)
        .unwrap();
    assert!(pretty.contains('\n'));
    let pretty_val: serde_json::Value = serde_json::from_str(&pretty).unwrap();
    assert_eq!(pretty_val, compact_val);

    let typed: SystemInfo = connection
        .call_typed("system", "info", r#"{"verbose":true}"#)
        .unwrap();
    assert_eq!(typed.uptime, 923);
    assert_eq!(typed.memory.total, 513433600);

    server_handle.join().unwrap();
}

const TEST_HELLO: &[u8] = &[
    0x00, 0x00, 0x00, 0x00, 0x2e, 0xb8, 0x63, 0xdb, 0x00, 0x00, 0x00, 0x04,
];

const TEST_TX: &[u8] = &[
    0x00, 0x05, 0x00, 0x01, 0x13, 0x33, 0x33, 0x37, 0x00, 0x00, 0x00, 0x1c, 0x03, 0x00, 0x00, 0x08,
    0x13, 0x33, 0x33, 0x37, 0x04, 0x00, 0x00, 0x09, 0x69, 0x6e, 0x66, 0x6f, 0x00, 0x00, 0x00, 0x00,
    0x07, 0x00, 0x00, 0x04,
];

const TEST_RX: &[&[u8]] = &[
    &[
        0x00, 0x02, 0x00, 0x01, 0x13, 0x33, 0x33, 0x37, 0x00, 0x00, 0x01, 0x34,
    ],
    &[
        0x03, 0x00, 0x00, 0x08, 0x13, 0x33, 0x33, 0x77, 0x07, 0x00, 0x01, 0x28, 0x85, 0x00, 0x00,
        0x14, 0x00, 0x09, 0x6c, 0x6f, 0x63, 0x61, 0x6c, 0x74, 0x69, 0x6d, 0x65, 0x00, 0x5f, 0x5e,
        0x51, 0x6c, 0x85, 0x00, 0x00, 0x14, 0x00, 0x06, 0x75, 0x70, 0x74, 0x69, 0x6d, 0x65, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x9b, 0x81, 0x00, 0x00, 0x30, 0x00, 0x04, 0x6c, 0x6f,
        0x61, 0x64, 0x00, 0x00, 0x85, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x85, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x85, 0x00,
        0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x82, 0x00, 0x00, 0x98, 0x00,
        0x06, 0x6d, 0x65, 0x6d, 0x6f, 0x72, 0x79, 0x00, 0x00, 0x00, 0x00, 0x84, 0x00, 0x00, 0x14,
        0x00, 0x05, 0x74, 0x6f, 0x74, 0x61, 0x6c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1e, 0x9a, 0x60,
        0x00, 0x84, 0x00, 0x00, 0x14, 0x00, 0x04, 0x66, 0x72, 0x65, 0x65, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x1c, 0x51, 0x30, 0x00, 0x84, 0x00, 0x00, 0x18, 0x00, 0x06, 0x73, 0x68, 0x61,
        0x72, 0x65, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x84, 0x00, 0x00, 0x18, 0x00, 0x08, 0x62, 0x75, 0x66, 0x66, 0x65, 0x72, 0x65, 0x64, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0d, 0x20, 0x00, 0x84, 0x00, 0x00, 0x18, 0x00, 0x09,
        0x61, 0x76, 0x61, 0x69, 0x6c, 0x61, 0x62, 0x6c, 0x65, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1b,
        0x17, 0xa0, 0x00, 0x84, 0x00, 0x00, 0x18, 0x00, 0x06, 0x63, 0x61, 0x63, 0x68, 0x65, 0x64,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x99, 0x70, 0x00, 0x82, 0x00, 0x00,
        0x34, 0x00, 0x04, 0x73, 0x77, 0x61, 0x70, 0x00, 0x00, 0x84, 0x00, 0x00, 0x14, 0x00, 0x05,
        0x74, 0x6f, 0x74, 0x61, 0x6c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x84,
        0x00, 0x00, 0x14, 0x00, 0x04, 0x66, 0x72, 0x65, 0x65, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
    ],
    &[
        0x00, 0x01, 0x00, 0x01, 0x13, 0x33, 0x33, 0x77, 0x00, 0x00, 0x00, 0x14,
    ],
    &[
        0x01, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x08, 0x85, 0x22, 0x59,
        0x61,
    ],
];
