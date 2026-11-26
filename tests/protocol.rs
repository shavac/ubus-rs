use serde::{Deserialize, Serialize};
#[cfg(feature = "json")]
use std::collections::HashMap;
use std::io::{Read, Write};
use std::mem::size_of;
use std::os::unix::net::UnixStream;
use std::time::Duration;
use ubus::*;

const TEST_HELLO: &[u8] = &[
    0x00, 0x00, 0x00, 0x00, 0x2e, 0xb8, 0x63, 0xdb, 0x00, 0x00, 0x00, 0x04,
];

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

fn write_status(stream: &mut UnixStream, seq: BEu16, peer: u32, status: i32) {
    let mut buf = [0u8; 32];
    let mut bb = BlobBuilder::from_bytes(&mut buf);
    bb.push_u32(BlobAttrId::STATUS.value(), status as u32)
        .unwrap();
    let len = bb.len();
    write_msg(
        stream,
        UbusMsgHeader {
            version: UbusMsgVersion::CURRENT,
            cmd_type: UbusCmdType::STATUS,
            sequence: seq,
            peer: peer.into(),
        },
        &buf[..len],
    );
}

#[test]
fn test_protocol_constants_status_and_connection_size() {
    assert_eq!(UbusCmdType::NOTIFY.value(), 10);
    assert_eq!(UbusCmdType::MONITOR.value(), 11);
    assert_eq!(UbusCmdType::INVOKE_FD.value(), 12);
    assert!(size_of::<Connection<UnixStream>>() <= 80);
    assert_eq!(core::mem::align_of::<BlobTag>(), 4);
    assert_eq!(size_of::<UbusMsgHeader>(), 8);
    assert_eq!(BEu16::from(0x1234u16).to_be_bytes(), [0x12, 0x34]);
    assert_eq!(BEu16::from_be_bytes([0x12, 0x34]).get(), 0x1234);
    assert_eq!(BEu32::from(0x1234_5678u32).to_be_bytes(), [0x12, 0x34, 0x56, 0x78]);
    assert_eq!(BEu32::from_be_bytes([0x12, 0x34, 0x56, 0x78]).get(), 0x1234_5678);

    assert_eq!(UbusStatus::OK.as_str(), "Success");
    assert_eq!(UbusStatus::METHOD_NOT_FOUND.as_str(), "Method not found");
    assert_eq!(UbusStatus::TIMEOUT.as_str(), "Request timed out");

    let err = UbusError::Status(UbusStatus::PERMISSION_DENIED.value());
    assert_eq!(err.status(), Some(UbusStatus::PERMISSION_DENIED));
    assert!(err.to_string().contains("Permission denied"));
    assert_eq!(UbusError::Timeout.status(), Some(UbusStatus::TIMEOUT));
}

#[test]
fn test_timeout_ping_and_no_reply() {
    let (client, mut server) = UnixStream::pair().unwrap();

    let server_handle = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();

        // 1. Ping -> reply STATUS(0)
        let (ping_hdr, ping_body) = read_msg(&mut server);
        assert_eq!(ping_hdr.cmd_type, UbusCmdType::PING);
        assert!(ping_body.is_empty());
        write_status(&mut server, ping_hdr.sequence, 0, 0);

        // 2. No-reply invoke -> verify NoReply(true) is set, send nothing back
        let (nr_hdr, nr_body) = read_msg(&mut server);
        assert_eq!(nr_hdr.cmd_type, UbusCmdType::INVOKE);
        let mut saw_no_reply = false;
        for attr in BlobIter::<UbusMsgAttr>::new(&nr_body) {
            if let UbusMsgAttr::NoReply(true) = attr {
                saw_no_reply = true;
            }
        }
        assert!(saw_no_reply);

        // 3. Stalled invoke -> client should hit Timeout without server sending anything
        let (stall_hdr, _) = read_msg(&mut server);
        assert_eq!(stall_hdr.cmd_type, UbusCmdType::INVOKE);
        std::thread::sleep(Duration::from_millis(80));
    });

    let mut conn = Connection::new(client).unwrap();
    conn.ping().unwrap();
    conn.invoke_no_reply(42, "notify_me", &[]).unwrap();

    let err = conn
        .invoke_with_timeout(42, "slow", &[], Some(Duration::from_millis(20)), |_| {})
        .unwrap_err();
    assert!(matches!(err, UbusError::Timeout));
    assert_eq!(err.status(), Some(UbusStatus::TIMEOUT));
    assert_eq!(conn.timeout(), None);

    server_handle.join().unwrap();
}

#[test]
fn test_to_blob_from_blob_roundtrip_and_invoke_serde() {
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    enum Mode {
        Auto,
        Custom(u32),
        Detailed { level: i16, label: String },
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct NestedCfg {
        tags: Vec<String>,
        retries: u16,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct RpcArgs {
        flag: bool,
        tiny: i8,
        port: u16,
        count: i32,
        big: u64,
        ratio: f64,
        name: String,
        #[serde(default)]
        missing: Option<u32>,
        present: Option<i32>,
        mode_a: Mode,
        mode_b: Mode,
        mode_c: Mode,
        nested: NestedCfg,
    }

    let input = RpcArgs {
        flag: true,
        tiny: -12,
        port: 8080,
        count: -999,
        big: 123_456_789_012,
        ratio: 3.5,
        name: "lan0".to_string(),
        missing: None,
        present: Some(77),
        mode_a: Mode::Auto,
        mode_b: Mode::Custom(55),
        mode_c: Mode::Detailed {
            level: -3,
            label: "hi".to_string(),
        },
        nested: NestedCfg {
            tags: vec!["a".to_string(), "b".to_string()],
            retries: 5,
        },
    };

    let bytes = to_blob(&input).unwrap();
    let decoded: RpcArgs = from_blob(&bytes).unwrap();
    assert_eq!(decoded, input);

    // End-to-end invoke_serde without json feature
    #[derive(Debug, Serialize)]
    struct Req<'a> {
        iface: &'a str,
        up: bool,
    }
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Resp {
        ok: bool,
        mtu: u32,
    }

    let (client, mut server) = UnixStream::pair().unwrap();
    let server_handle = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();
        let (hdr, body) = read_msg(&mut server);
        assert_eq!(hdr.cmd_type, UbusCmdType::INVOKE);
        for attr in BlobIter::<UbusMsgAttr>::new(&body) {
            if let UbusMsgAttr::Data(d) = attr {
                #[derive(Deserialize)]
                struct WireReq<'a> {
                    iface: &'a str,
                    up: bool,
                }
                let r: WireReq = from_blob(d).unwrap();
                assert_eq!(r.iface, "eth0");
                assert!(r.up);
            }
        }

        let resp_blob = to_blob(&Resp {
            ok: true,
            mtu: 1500,
        })
        .unwrap();
        let mut data_buf = [0u8; 128];
        let mut db = BlobBuilder::from_bytes(&mut data_buf);
        db.push_u32(BlobAttrId::OBJID.value(), 99).unwrap();
        db.push_slice(BlobAttrId::DATA.value(), &resp_blob).unwrap();
        let dlen = db.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: hdr.sequence,
                peer: 99.into(),
            },
            &data_buf[..dlen],
        );
        write_status(&mut server, hdr.sequence, 99, 0);
    });

    let mut conn = Connection::new(client).unwrap();
    let resp: Resp = conn
        .invoke_serde(
            99,
            "set_state",
            &Req {
                iface: "eth0",
                up: true,
            },
        )
        .unwrap();
    assert_eq!(
        resp,
        Resp {
            ok: true,
            mtu: 1500
        }
    );
    server_handle.join().unwrap();
}

#[cfg(feature = "json")]
#[test]
fn test_nested_json_args_and_dynamic_policy() {
    let mut policy = HashMap::new();
    policy.insert("command", BlobMsgType::STRING);
    policy.insert("params", BlobMsgType::ARRAY);
    policy.insert("env", BlobMsgType::TABLE);

    let mut methods = HashMap::new();
    methods.insert(
        "exec",
        Method {
            name: "exec",
            policy,
        },
    );
    methods.insert(
        "dynamic",
        Method {
            name: "dynamic",
            policy: HashMap::new(),
        },
    );

    let obj = UbusObject {
        path: "file",
        id: 10,
        ty: 20,
        methods,
    };

    let bytes = obj
        .args_from_json(
            "exec",
            r#"{"command":"/bin/sh","params":["-c","echo ok",123],"env":{"PATH":"/bin","DEBUG":true},"ignored":99}"#,
        )
        .unwrap();

    #[derive(Deserialize, PartialEq, Debug)]
    struct ExecArgs<'a> {
        command: &'a str,
        params: Vec<serde_json::Value>,
        env: HashMap<&'a str, serde_json::Value>,
        #[serde(default)]
        ignored: Option<i32>,
    }
    let decoded: ExecArgs = from_blob(&bytes).unwrap();
    assert_eq!(decoded.command, "/bin/sh");
    assert_eq!(
        decoded.params,
        vec![
            serde_json::json!("-c"),
            serde_json::json!("echo ok"),
            serde_json::json!(123)
        ]
    );
    assert_eq!(decoded.env.get("PATH"), Some(&serde_json::json!("/bin")));
    assert_eq!(decoded.env.get("DEBUG"), Some(&serde_json::json!(true)));
    assert_eq!(decoded.ignored, None);

    // Empty-policy method encodes JSON fields dynamically
    let dyn_bytes = obj
        .args_from_json("dynamic", r#"{"a":1,"b":["x",false],"c":{"k":2.5}}"#)
        .unwrap();
    let dyn_json = serde_json::to_string(&BlobTable(&dyn_bytes)).unwrap();
    let dyn_val: serde_json::Value = serde_json::from_str(&dyn_json).unwrap();
    assert_eq!(dyn_val["a"], 1);
    assert_eq!(dyn_val["b"], serde_json::json!(["x", false]));
    assert_eq!(dyn_val["c"]["k"], 2.5);

    // json_to_blob standalone helper
    let raw = json_to_blob(r#"{"hello":"world","arr":[1,2]}"#).unwrap();
    let raw_val: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&BlobTable(&raw)).unwrap()).unwrap();
    assert_eq!(raw_val["hello"], "world");
    assert_eq!(raw_val["arr"], serde_json::json!([1, 2]));
}

#[test]
fn test_server_add_object_dispatch_and_remove() {
    #[derive(Serialize)]
    struct Pong<'a> {
        msg: &'a str,
    }

    let (client, mut server) = UnixStream::pair().unwrap();
    let mock_ubusd = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();

        // 1. ADD_OBJECT
        let (add_hdr, add_body) = read_msg(&mut server);
        assert_eq!(add_hdr.cmd_type, UbusCmdType::ADD_OBJECT);
        let mut saw_path = false;
        let mut saw_sig = false;
        for attr in BlobIter::<UbusMsgAttr>::new(&add_body) {
            match attr {
                UbusMsgAttr::ObjPath(p) => {
                    assert_eq!(p, "my.service");
                    saw_path = true;
                }
                UbusMsgAttr::Signature(sig) => {
                    let methods: Vec<Blob> = sig.collect();
                    assert_eq!(methods.len(), 1);
                    let (m_name, _) = methods[0].into_named_payload().unwrap();
                    assert_eq!(m_name, "echo");
                    saw_sig = true;
                }
                _ => {}
            }
        }
        assert!(saw_path && saw_sig);

        let mut add_reply = [0u8; 64];
        let mut ab = BlobBuilder::from_bytes(&mut add_reply);
        ab.push_u32(BlobAttrId::OBJID.value(), 0x1234).unwrap();
        ab.push_u32(BlobAttrId::OBJTYPE.value(), 0x5678).unwrap();
        let alen = ab.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: add_hdr.sequence,
                peer: 0.into(),
            },
            &add_reply[..alen],
        );
        write_status(&mut server, add_hdr.sequence, 0, 0);

        // 2. Forward an incoming INVOKE to the registered object
        let mut inv_buf = [0u8; 128];
        let mut ib = BlobBuilder::from_bytes(&mut inv_buf);
        ib.push_u32(BlobAttrId::OBJID.value(), 0x1234).unwrap();
        ib.push_str(BlobAttrId::METHOD.value(), "echo").unwrap();
        let arg_bytes = to_blob(&Pong { msg: "hi" }).unwrap();
        ib.push_slice(BlobAttrId::DATA.value(), &arg_bytes).unwrap();
        let ilen = ib.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::INVOKE,
                sequence: 77u16.into(),
                peer: 0x9999.into(),
            },
            &inv_buf[..ilen],
        );

        // Expect DATA + STATUS reply from server handler
        let (rep_data_hdr, rep_data_body) = read_msg(&mut server);
        assert_eq!(rep_data_hdr.cmd_type, UbusCmdType::DATA);
        assert_eq!(u16::from(rep_data_hdr.sequence), 77);
        assert_eq!(u32::from(rep_data_hdr.peer), 0x9999);
        let mut got_pong = false;
        for attr in BlobIter::<UbusMsgAttr>::new(&rep_data_body) {
            if let UbusMsgAttr::Data(d) = attr {
                #[derive(Deserialize)]
                struct ReplyView<'a> {
                    msg: &'a str,
                }
                let rv: ReplyView = from_blob(d).unwrap();
                assert_eq!(rv.msg, "hi!");
                got_pong = true;
            }
        }
        assert!(got_pong);

        let (rep_st_hdr, rep_st_body) = read_msg(&mut server);
        assert_eq!(rep_st_hdr.cmd_type, UbusCmdType::STATUS);
        assert_eq!(u16::from(rep_st_hdr.sequence), 77);
        let mut st_ok = false;
        for attr in BlobIter::<UbusMsgAttr>::new(&rep_st_body) {
            if let UbusMsgAttr::Status(0) = attr {
                st_ok = true;
            }
        }
        assert!(st_ok);

        // 3. REMOVE_OBJECT
        let (rem_hdr, rem_body) = read_msg(&mut server);
        assert_eq!(rem_hdr.cmd_type, UbusCmdType::REMOVE_OBJECT);
        let mut rem_id = None;
        for attr in BlobIter::<UbusMsgAttr>::new(&rem_body) {
            if let UbusMsgAttr::ObjId(id) = attr {
                rem_id = Some(id);
            }
        }
        assert_eq!(rem_id, Some(0x1234));
        write_status(&mut server, rem_hdr.sequence, 0, 0);
    });

    let mut conn = Connection::new(client).unwrap();
    let reg = conn
        .add_object(&ObjectSpec {
            path: Some("my.service"),
            methods: &[MethodSpec {
                name: "echo",
                policy: &[("msg", BlobMsgType::STRING)],
            }],
        })
        .unwrap();
    assert_eq!(
        reg,
        RegisteredObject {
            id: 0x1234,
            ty: 0x5678
        }
    );

    let handled = conn
        .dispatch_one(|req| {
            assert_eq!(req.obj_id, reg.id);
            assert_eq!(req.method, "echo");
            #[derive(Deserialize)]
            struct In<'a> {
                msg: &'a str,
            }
            let input: In = from_blob(req.data).unwrap();
            let reply = format!("{}!", input.msg);
            Ok(RequestReply::Data(to_blob(&Pong { msg: &reply }).unwrap()))
        })
        .unwrap();
    assert!(handled);

    conn.remove_object(reg.id).unwrap();
    mock_ubusd.join().unwrap();
}

#[test]
fn test_events_send_and_listen() {
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct LinkEvent {
        up: bool,
    }

    let (client, mut server) = UnixStream::pair().unwrap();
    let mock_ubusd = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();

        // 1. send_event_serde -> INVOKE on UBUS_SYSTEM_OBJECT_EVENT (1), method "send"
        let (send_hdr, send_body) = read_msg(&mut server);
        assert_eq!(send_hdr.cmd_type, UbusCmdType::INVOKE);
        assert_eq!(u32::from(send_hdr.peer), UBUS_SYSTEM_OBJECT_EVENT);
        for attr in BlobIter::<UbusMsgAttr>::new(&send_body) {
            match attr {
                UbusMsgAttr::Method(m) => assert_eq!(m, "send"),
                UbusMsgAttr::Data(d) => {
                    #[derive(Deserialize)]
                    struct Ev<'a> {
                        id: &'a str,
                        data: LinkEvent,
                    }
                    let ev: Ev = from_blob(d).unwrap();
                    assert_eq!(ev.id, "net.link");
                    assert_eq!(ev.data, LinkEvent { up: true });
                }
                _ => {}
            }
        }
        write_status(&mut server, send_hdr.sequence, UBUS_SYSTEM_OBJECT_EVENT, 0);

        // 2. listen_events -> ADD_OBJECT (anonymous)
        let (add_hdr, add_body) = read_msg(&mut server);
        assert_eq!(add_hdr.cmd_type, UbusCmdType::ADD_OBJECT);
        assert!(add_body.is_empty());
        let mut add_reply = [0u8; 32];
        let mut ab = BlobBuilder::from_bytes(&mut add_reply);
        ab.push_u32(BlobAttrId::OBJID.value(), 500).unwrap();
        let alen = ab.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: add_hdr.sequence,
                peer: 0.into(),
            },
            &add_reply[..alen],
        );
        write_status(&mut server, add_hdr.sequence, 0, 0);

        // 3. listen_events -> INVOKE on event object (1), method "register"
        let (reg_hdr, reg_body) = read_msg(&mut server);
        assert_eq!(reg_hdr.cmd_type, UbusCmdType::INVOKE);
        assert_eq!(u32::from(reg_hdr.peer), UBUS_SYSTEM_OBJECT_EVENT);
        for attr in BlobIter::<UbusMsgAttr>::new(&reg_body) {
            match attr {
                UbusMsgAttr::Method(m) => assert_eq!(m, "register"),
                UbusMsgAttr::Data(d) => {
                    #[derive(Deserialize)]
                    struct Reg<'a> {
                        object: i32,
                        pattern: &'a str,
                    }
                    let r: Reg = from_blob(d).unwrap();
                    assert_eq!(r.object, 500);
                    assert_eq!(r.pattern, "net.*");
                }
                _ => {}
            }
        }
        write_status(&mut server, reg_hdr.sequence, UBUS_SYSTEM_OBJECT_EVENT, 0);

        // 4. Push an incoming event INVOKE to object 500
        let ev_payload = to_blob(&LinkEvent { up: false }).unwrap();
        let mut inv_buf = [0u8; 128];
        let mut ib = BlobBuilder::from_bytes(&mut inv_buf);
        ib.push_u32(BlobAttrId::OBJID.value(), 500).unwrap();
        ib.push_str(BlobAttrId::METHOD.value(), "net.link")
            .unwrap();
        ib.push_slice(BlobAttrId::DATA.value(), &ev_payload)
            .unwrap();
        let ilen = ib.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::INVOKE,
                sequence: 88u16.into(),
                peer: 1.into(),
            },
            &inv_buf[..ilen],
        );

        // Expect STATUS(0) from listener
        let (ack_hdr, _) = read_msg(&mut server);
        assert_eq!(ack_hdr.cmd_type, UbusCmdType::STATUS);
        assert_eq!(u16::from(ack_hdr.sequence), 88);

        // 5. REMOVE_OBJECT(500)
        let (rem_hdr, _) = read_msg(&mut server);
        assert_eq!(rem_hdr.cmd_type, UbusCmdType::REMOVE_OBJECT);
        write_status(&mut server, rem_hdr.sequence, 0, 0);
    });

    let mut conn = Connection::new(client).unwrap();
    conn.send_event_serde("net.link", &LinkEvent { up: true })
        .unwrap();

    let mut received = Vec::new();
    conn.listen_events("net.*", |ev_name, bi| {
        let ev: LinkEvent = bi.deserialize().unwrap();
        received.push((ev_name.to_string(), ev));
        false
    })
    .unwrap();
    assert_eq!(
        received,
        vec![("net.link".to_string(), LinkEvent { up: false })]
    );
    mock_ubusd.join().unwrap();
}

#[test]
fn test_subscribe_unsubscribe_and_remove_object_callback() {
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct SensorMsg {
        temp: i32,
    }
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct SubAck {
        seen: bool,
    }

    let (client, mut server) = UnixStream::pair().unwrap();
    let mock_ubusd = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();

        // 1. register_subscriber() -> ADD_OBJECT (anonymous)
        let (add_hdr, add_body) = read_msg(&mut server);
        assert_eq!(add_hdr.cmd_type, UbusCmdType::ADD_OBJECT);
        assert!(add_body.is_empty());
        let mut add_reply = [0u8; 32];
        let mut ab = BlobBuilder::from_bytes(&mut add_reply);
        ab.push_u32(BlobAttrId::OBJID.value(), 701).unwrap();
        let alen = ab.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: add_hdr.sequence,
                peer: 0.into(),
            },
            &add_reply[..alen],
        );
        write_status(&mut server, add_hdr.sequence, 0, 0);

        // 2. subscribe(701, 0x2000) -> SUBSCRIBE(ObjId=701, Target=0x2000)
        let (sub_hdr, sub_body) = read_msg(&mut server);
        assert_eq!(sub_hdr.cmd_type, UbusCmdType::SUBSCRIBE);
        let mut got_sub = None;
        let mut got_target = None;
        for attr in BlobIter::<UbusMsgAttr>::new(&sub_body) {
            match attr {
                UbusMsgAttr::ObjId(id) => got_sub = Some(id),
                UbusMsgAttr::Target(t) => got_target = Some(t),
                _ => {}
            }
        }
        assert_eq!(got_sub, Some(701));
        assert_eq!(got_target, Some(0x2000));
        write_status(&mut server, sub_hdr.sequence, 0, 0);

        // 3. Push incoming notification INVOKE to subscriber 701
        let n_payload = to_blob(&SensorMsg { temp: 42 }).unwrap();
        let mut inv_buf = [0u8; 128];
        let mut ib = BlobBuilder::from_bytes(&mut inv_buf);
        ib.push_u32(BlobAttrId::OBJID.value(), 701).unwrap();
        ib.push_str(BlobAttrId::METHOD.value(), "reading")
            .unwrap();
        ib.push_slice(BlobAttrId::DATA.value(), &n_payload)
            .unwrap();
        let ilen = ib.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::INVOKE,
                sequence: 91u16.into(),
                peer: 0x2000.into(),
            },
            &inv_buf[..ilen],
        );

        // Expect subscriber DATA + STATUS reply
        let (rep_data_hdr, rep_data_body) = read_msg(&mut server);
        assert_eq!(rep_data_hdr.cmd_type, UbusCmdType::DATA);
        assert_eq!(u16::from(rep_data_hdr.sequence), 91);
        for attr in BlobIter::<UbusMsgAttr>::new(&rep_data_body) {
            if let UbusMsgAttr::Data(d) = attr {
                let ack: SubAck = from_blob(d).unwrap();
                assert_eq!(ack, SubAck { seen: true });
            }
        }
        let (rep_st_hdr, _) = read_msg(&mut server);
        assert_eq!(rep_st_hdr.cmd_type, UbusCmdType::STATUS);
        assert_eq!(u16::from(rep_st_hdr.sequence), 91);

        // 4. Push REMOVE_OBJECT notification to subscriber 701 (target 0x2000 gone)
        let mut rem_notif_buf = [0u8; 64];
        let mut rb = BlobBuilder::from_bytes(&mut rem_notif_buf);
        rb.push_u32(BlobAttrId::OBJID.value(), 701).unwrap();
        rb.push_u32(BlobAttrId::TARGET.value(), 0x2000).unwrap();
        let rlen = rb.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::REMOVE_OBJECT,
                sequence: 92u16.into(),
                peer: 0.into(),
            },
            &rem_notif_buf[..rlen],
        );

        // 5. unsubscribe(701, 0x2000)
        let (unsub_hdr, unsub_body) = read_msg(&mut server);
        assert_eq!(unsub_hdr.cmd_type, UbusCmdType::UNSUBSCRIBE);
        let mut u_sub = None;
        let mut u_target = None;
        for attr in BlobIter::<UbusMsgAttr>::new(&unsub_body) {
            match attr {
                UbusMsgAttr::ObjId(id) => u_sub = Some(id),
                UbusMsgAttr::Target(t) => u_target = Some(t),
                _ => {}
            }
        }
        assert_eq!(u_sub, Some(701));
        assert_eq!(u_target, Some(0x2000));
        write_status(&mut server, unsub_hdr.sequence, 0, 0);

        // 6. remove_object(701)
        let (rem_hdr, _) = read_msg(&mut server);
        assert_eq!(rem_hdr.cmd_type, UbusCmdType::REMOVE_OBJECT);
        write_status(&mut server, rem_hdr.sequence, 0, 0);
    });

    let mut conn = Connection::new(client).unwrap();
    let sub = conn.register_subscriber().unwrap();
    assert_eq!(sub.id, 701);

    conn.subscribe(sub.id, 0x2000).unwrap();

    let mut seen_reading = false;
    let handled = conn
        .dispatch_event(|ev| match ev {
            IncomingEvent::Invoke(req) => {
                assert_eq!(req.obj_id, 701);
                assert_eq!(req.method, "reading");
                let msg: SensorMsg = from_blob(req.data).unwrap();
                assert_eq!(msg.temp, 42);
                seen_reading = true;
                Ok(RequestReply::Data(to_blob(&SubAck { seen: true }).unwrap()))
            }
            _ => panic!("unexpected event: {:?}", ev),
        })
        .unwrap();
    assert!(handled && seen_reading);

    let mut saw_removed = false;
    let handled_rem = conn
        .dispatch_event(|ev| match ev {
            IncomingEvent::ObjectRemoved {
                subscriber_id,
                target_id,
            } => {
                assert_eq!(subscriber_id, 701);
                assert_eq!(target_id, 0x2000);
                saw_removed = true;
                Ok(RequestReply::Status(UbusStatus::OK))
            }
            _ => panic!("unexpected event: {:?}", ev),
        })
        .unwrap();
    assert!(handled_rem && saw_removed);

    conn.unsubscribe(sub.id, 0x2000).unwrap();
    conn.remove_object(sub.id).unwrap();
    mock_ubusd.join().unwrap();
}

#[test]
fn test_publisher_active_callback_and_notify() {
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Alert {
        level: u8,
    }
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct SubReply {
        ack: bool,
    }

    let (client, mut server) = UnixStream::pair().unwrap();
    let mock_ubusd = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();

        // 1. add_object
        let (add_hdr, _) = read_msg(&mut server);
        assert_eq!(add_hdr.cmd_type, UbusCmdType::ADD_OBJECT);
        let mut add_reply = [0u8; 32];
        let mut ab = BlobBuilder::from_bytes(&mut add_reply);
        ab.push_u32(BlobAttrId::OBJID.value(), 801).unwrap();
        ab.push_u32(BlobAttrId::OBJTYPE.value(), 0xaa).unwrap();
        let alen = ab.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: add_hdr.sequence,
                peer: 0.into(),
            },
            &add_reply[..alen],
        );
        write_status(&mut server, add_hdr.sequence, 0, 0);

        // 2. ubusd sends NOTIFY(ACTIVE = true) to publisher
        let mut act_buf = [0u8; 64];
        let mut actb = BlobBuilder::from_bytes(&mut act_buf);
        actb.push_u32(BlobAttrId::OBJID.value(), 801).unwrap();
        actb.push_bool(BlobAttrId::ACTIVE.value(), true).unwrap();
        let act_len = actb.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::NOTIFY,
                sequence: 10u16.into(),
                peer: 0.into(),
            },
            &act_buf[..act_len],
        );

        // 3. Publisher sends notify_no_reply_serde
        let (nr_hdr, nr_body) = read_msg(&mut server);
        assert_eq!(nr_hdr.cmd_type, UbusCmdType::NOTIFY);
        assert_eq!(nr_hdr.cmd_type.value(), 0x0a);
        let mut saw_no_reply = false;
        let mut saw_obj = None;
        let mut saw_method = None;
        for attr in BlobIter::<UbusMsgAttr>::new(&nr_body) {
            match attr {
                UbusMsgAttr::ObjId(id) => saw_obj = Some(id),
                UbusMsgAttr::Method(m) => saw_method = Some(m.to_string()),
                UbusMsgAttr::NoReply(true) => saw_no_reply = true,
                _ => {}
            }
        }
        assert!(saw_no_reply);
        assert_eq!(saw_obj, Some(801));
        assert_eq!(saw_method.as_deref(), Some("warn"));

        // 4. Publisher sends notify_serde with 2 subscribers (9001, 9002)
        let (n_hdr, _) = read_msg(&mut server);
        assert_eq!(n_hdr.cmd_type, UbusCmdType::NOTIFY);

        // ubusd sends DATA(Subscribers = [9001, 9002])
        let mut subs_buf = [0u8; 32];
        let mut sb = BlobBuilder::from_bytes(&mut subs_buf);
        sb.push_u32(0, 9001).unwrap();
        sb.push_u32(0, 9002).unwrap();
        let subs_len = sb.len();

        let mut subs_attr_buf = [0u8; 64];
        let mut sab = BlobBuilder::from_bytes(&mut subs_attr_buf);
        sab.push_slice(BlobAttrId::SUBSCRIBERS.value(), &subs_buf[..subs_len])
            .unwrap();
        let sab_len = sab.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: n_hdr.sequence,
                peer: 0.into(),
            },
            &subs_attr_buf[..sab_len],
        );
        // ubusd finishes dispatching -> STATUS(0) with peer=0
        write_status(&mut server, n_hdr.sequence, 0, 0);

        // Subscriber 9001 sends DATA + STATUS(OK)
        let sub_data = to_blob(&SubReply { ack: true }).unwrap();
        let mut d_buf = [0u8; 64];
        let mut db = BlobBuilder::from_bytes(&mut d_buf);
        db.push_u32(BlobAttrId::OBJID.value(), 9001).unwrap();
        db.push_slice(BlobAttrId::DATA.value(), &sub_data).unwrap();
        let d_len = db.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: n_hdr.sequence,
                peer: 9001.into(),
            },
            &d_buf[..d_len],
        );
        write_status(&mut server, n_hdr.sequence, 9001, UbusStatus::OK.value());

        // Subscriber 9002 sends STATUS(PERMISSION_DENIED)
        write_status(
            &mut server,
            n_hdr.sequence,
            9002,
            UbusStatus::PERMISSION_DENIED.value(),
        );

        // 5. notify with 0 subscribers -> only STATUS(0, peer=0)
        let (n0_hdr, _) = read_msg(&mut server);
        assert_eq!(n0_hdr.cmd_type, UbusCmdType::NOTIFY);
        write_status(&mut server, n0_hdr.sequence, 0, 0);

        // 6. ubusd sends NOTIFY(ACTIVE = false)
        let mut deact_buf = [0u8; 64];
        let mut deactb = BlobBuilder::from_bytes(&mut deact_buf);
        deactb.push_u32(BlobAttrId::OBJID.value(), 801).unwrap();
        deactb.push_bool(BlobAttrId::ACTIVE.value(), false).unwrap();
        let deact_len = deactb.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::NOTIFY,
                sequence: 11u16.into(),
                peer: 0.into(),
            },
            &deact_buf[..deact_len],
        );
    });

    let mut conn = Connection::new(client).unwrap();
    let reg = conn
        .add_object(&ObjectSpec {
            path: Some("sensor"),
            methods: &[],
        })
        .unwrap();
    assert_eq!(reg.id, 801);

    // Expect ACTIVE = true
    let mut active_state = false;
    let handled_act = conn
        .dispatch_event(|ev| match ev {
            IncomingEvent::SubscribeActive { obj_id, active } => {
                assert_eq!(obj_id, 801);
                active_state = active;
                Ok(RequestReply::Status(UbusStatus::OK))
            }
            _ => panic!("unexpected event"),
        })
        .unwrap();
    assert!(handled_act && active_state);

    conn.notify_no_reply_serde(reg.id, "warn", &Alert { level: 2 })
        .unwrap();

    let mut replies = Vec::new();
    let mut statuses = Vec::new();
    conn.notify_serde(
        reg.id,
        "alert",
        &Alert { level: 5 },
        |sub_id, bi| {
            let r: SubReply = bi.deserialize().unwrap();
            replies.push((sub_id, r));
        },
        |sub_id, st| {
            statuses.push((sub_id, st));
        },
    )
    .unwrap();
    assert_eq!(replies, vec![(9001, SubReply { ack: true })]);
    assert_eq!(
        statuses,
        vec![
            (9001, UbusStatus::OK),
            (9002, UbusStatus::PERMISSION_DENIED),
        ]
    );

    // 0-subscriber notify finishes immediately on peer=0 STATUS(0)
    conn.notify(reg.id, "empty", &[], |_, _| {}, |_, _| {})
        .unwrap();

    // Expect ACTIVE = false
    let handled_deact = conn
        .dispatch_event(|ev| match ev {
            IncomingEvent::SubscribeActive { obj_id, active } => {
                assert_eq!(obj_id, 801);
                active_state = active;
                Ok(RequestReply::Status(UbusStatus::OK))
            }
            _ => panic!("unexpected event"),
        })
        .unwrap();
    assert!(handled_deact && !active_state);

    mock_ubusd.join().unwrap();
}

#[test]
fn test_pending_queue_bidirectional_multiplexing() {
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct EchoMsg {
        val: i32,
    }

    let (client, mut server) = UnixStream::pair().unwrap();
    let mock_ubusd = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();

        // Client calls invoke(10, "get", ...)
        let (inv_hdr, _) = read_msg(&mut server);
        assert_eq!(inv_hdr.cmd_type, UbusCmdType::INVOKE);

        // Before replying to client's invoke, inject:
        // 1) An out-of-band PING (should be answered immediately)
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::PING,
                sequence: 500u16.into(),
                peer: 0.into(),
            },
            &[],
        );
        let (pong_hdr, _) = read_msg(&mut server);
        assert_eq!(pong_hdr.cmd_type, UbusCmdType::STATUS);
        assert_eq!(u16::from(pong_hdr.sequence), 500);

        // 2) An incoming RPC INVOKE targeting our registered object 42
        let in_payload = to_blob(&EchoMsg { val: 7 }).unwrap();
        let mut in_buf = [0u8; 128];
        let mut ib = BlobBuilder::from_bytes(&mut in_buf);
        ib.push_u32(BlobAttrId::OBJID.value(), 42).unwrap();
        ib.push_str(BlobAttrId::METHOD.value(), "ping_obj")
            .unwrap();
        ib.push_slice(BlobAttrId::DATA.value(), &in_payload)
            .unwrap();
        let ilen = ib.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::INVOKE,
                sequence: 501u16.into(),
                peer: 0x3333.into(),
            },
            &in_buf[..ilen],
        );

        // 3) A NOTIFY(ACTIVE = true) for object 42
        let mut act_buf = [0u8; 64];
        let mut ab = BlobBuilder::from_bytes(&mut act_buf);
        ab.push_u32(BlobAttrId::OBJID.value(), 42).unwrap();
        ab.push_bool(BlobAttrId::ACTIVE.value(), true).unwrap();
        let alen = ab.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::NOTIFY,
                sequence: 502u16.into(),
                peer: 0.into(),
            },
            &act_buf[..alen],
        );

        // Finally reply DATA + STATUS to client's invoke
        let out_payload = to_blob(&EchoMsg { val: 99 }).unwrap();
        let mut out_buf = [0u8; 128];
        let mut ob = BlobBuilder::from_bytes(&mut out_buf);
        ob.push_u32(BlobAttrId::OBJID.value(), 10).unwrap();
        ob.push_slice(BlobAttrId::DATA.value(), &out_payload)
            .unwrap();
        let olen = ob.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: inv_hdr.sequence,
                peer: 10.into(),
            },
            &out_buf[..olen],
        );
        write_status(&mut server, inv_hdr.sequence, 10, 0);

        // After client runs dispatch_pending, expect reply DATA + STATUS for seq 501
        let (rep_data_hdr, rep_data_body) = read_msg(&mut server);
        assert_eq!(rep_data_hdr.cmd_type, UbusCmdType::DATA);
        assert_eq!(u16::from(rep_data_hdr.sequence), 501);
        for attr in BlobIter::<UbusMsgAttr>::new(&rep_data_body) {
            if let UbusMsgAttr::Data(d) = attr {
                let m: EchoMsg = from_blob(d).unwrap();
                assert_eq!(m, EchoMsg { val: 8 });
            }
        }
        let (rep_st_hdr, _) = read_msg(&mut server);
        assert_eq!(rep_st_hdr.cmd_type, UbusCmdType::STATUS);
        assert_eq!(u16::from(rep_st_hdr.sequence), 501);
    });

    let mut conn = Connection::new(client).unwrap();
    let res: EchoMsg = conn.invoke_typed(10, "get", &[]).unwrap();
    assert_eq!(res, EchoMsg { val: 99 });
    assert_eq!(conn.pending_count(), 2);

    let mut saw_invoke = false;
    let mut saw_active = false;
    let count = conn
        .dispatch_pending(|ev| match ev {
            IncomingEvent::Invoke(req) => {
                assert_eq!(req.obj_id, 42);
                assert_eq!(req.method, "ping_obj");
                let m: EchoMsg = from_blob(req.data).unwrap();
                saw_invoke = true;
                Ok(RequestReply::Data(
                    to_blob(&EchoMsg { val: m.val + 1 }).unwrap(),
                ))
            }
            IncomingEvent::SubscribeActive { obj_id, active } => {
                assert_eq!(obj_id, 42);
                assert!(active);
                saw_active = true;
                Ok(RequestReply::Status(UbusStatus::OK))
            }
            _ => panic!("unexpected event"),
        })
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(conn.pending_count(), 0);
    assert!(saw_invoke && saw_active);

    mock_ubusd.join().unwrap();
}

#[test]
fn test_exact_deadline_timeout_with_noise() {
    let (client, mut server) = UnixStream::pair().unwrap();
    let server_handle = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();
        let (_inv_hdr, _) = read_msg(&mut server);
        // Flood unrelated PINGs every 12ms so a single read timeout never fires;
        // only the overall Instant deadline can stop the loop.
        for seq in 1u16..=15 {
            std::thread::sleep(Duration::from_millis(12));
            let ping = UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::PING,
                sequence: seq.into(),
                peer: 0.into(),
            };
            let tag = BlobTag::new(0, BlobTag::SIZE, false).unwrap();
            if server.write_all(&ping.to_bytes()).is_err() {
                break;
            }
            if server.write_all(&tag.to_bytes()).is_err() {
                break;
            }
            // Drain client's ping status reply
            let mut reply = [0u8; 24];
            if server.read_exact(&mut reply).is_err() {
                break;
            }
        }
    });

    let mut conn = Connection::new(client).unwrap();
    let start = std::time::Instant::now();
    let err = conn
        .invoke_with_timeout(1, "slow", &[], Some(Duration::from_millis(55)), |_| {})
        .unwrap_err();
    let elapsed = start.elapsed();
    assert!(matches!(err, UbusError::Timeout));
    assert!(elapsed >= Duration::from_millis(45));
    assert!(elapsed < Duration::from_millis(250));
    assert_eq!(conn.timeout(), None);

    drop(conn);
    server_handle.join().unwrap();
}

#[test]
fn test_as_raw_fd_and_reconnect_and_restore() {
    use std::os::unix::io::{AsFd, AsRawFd};

    let (client1, mut server1) = UnixStream::pair().unwrap();
    let (client2, mut server2) = UnixStream::pair().unwrap();

    let h1 = std::thread::spawn(move || {
        server1.write_all(TEST_HELLO).unwrap();
        // Close immediately after hello to simulate ubusd restart
    });

    let h2 = std::thread::spawn(move || {
        // New HELLO with a different peer ID
        let hello2 = [
            0x00, 0x00, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44, 0x00, 0x00, 0x00, 0x04,
        ];
        server2.write_all(&hello2).unwrap();

        let (add_hdr, _) = read_msg(&mut server2);
        assert_eq!(add_hdr.cmd_type, UbusCmdType::ADD_OBJECT);
        let mut add_reply = [0u8; 32];
        let mut ab = BlobBuilder::from_bytes(&mut add_reply);
        ab.push_u32(BlobAttrId::OBJID.value(), 0x8888).unwrap();
        ab.push_u32(BlobAttrId::OBJTYPE.value(), 0x9999).unwrap();
        let alen = ab.len();
        write_msg(
            &mut server2,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: add_hdr.sequence,
                peer: 0.into(),
            },
            &add_reply[..alen],
        );
        write_status(&mut server2, add_hdr.sequence, 0, 0);
    });

    let mut conn = Connection::new(client1).unwrap();
    assert_eq!(conn.peer_id(), 0x2eb8_63db);
    assert!(conn.as_raw_fd() >= 0);
    let _ = conn.as_fd();
    conn.set_nonblocking(false).unwrap();
    h1.join().unwrap();

    let specs = [ObjectSpec {
        path: Some("restored.svc"),
        methods: &[MethodSpec {
            name: "status",
            policy: &[],
        }],
    }];
    let regs = conn.reconnect_and_restore(client2, &specs).unwrap();
    assert_eq!(conn.peer_id(), 0x1122_3344);
    assert_eq!(conn.pending_count(), 0);
    assert_eq!(
        regs,
        vec![RegisteredObject {
            id: 0x8888,
            ty: 0x9999,
        }]
    );
    h2.join().unwrap();
}

#[test]
fn test_monitor_mode() {
    let (client, mut server) = UnixStream::pair().unwrap();
    let server_handle = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();

        let (mon_hdr, mon_body) = read_msg(&mut server);
        assert_eq!(mon_hdr.cmd_type, UbusCmdType::MONITOR);
        assert!(mon_body.is_empty());
        write_status(&mut server, mon_hdr.sequence, 0, 0);

        // Packet 1: INVOKE frame
        let inner = [1u8, 2, 3, 4];
        let mut buf1 = [0u8; 128];
        let mut bb1 = BlobBuilder::from_bytes(&mut buf1);
        bb1.push_u32(MonitorAttrId::CLIENT.value(), 0x100).unwrap();
        bb1.push_u32(MonitorAttrId::PEER.value(), 0x200).unwrap();
        bb1.push_bool(MonitorAttrId::SEND.value(), true).unwrap();
        bb1.push_u32(MonitorAttrId::SEQ.value(), 7).unwrap();
        bb1.push_u32(MonitorAttrId::TYPE.value(), UbusCmdType::INVOKE.value() as u32)
            .unwrap();
        bb1.push_slice(MonitorAttrId::DATA.value(), &inner).unwrap();
        let len1 = bb1.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::MONITOR,
                sequence: 0.into(),
                peer: 0.into(),
            },
            &buf1[..len1],
        );

        // Packet 2: STATUS frame
        let mut buf2 = [0u8; 128];
        let mut bb2 = BlobBuilder::from_bytes(&mut buf2);
        bb2.push_u32(MonitorAttrId::CLIENT.value(), 0x200).unwrap();
        bb2.push_u32(MonitorAttrId::PEER.value(), 0x100).unwrap();
        bb2.push_bool(MonitorAttrId::SEND.value(), false).unwrap();
        bb2.push_u32(MonitorAttrId::SEQ.value(), 7).unwrap();
        bb2.push_u32(MonitorAttrId::TYPE.value(), UbusCmdType::STATUS.value() as u32)
            .unwrap();
        bb2.push_u32(
            MonitorAttrId::STATUS.value(),
            UbusStatus::PERMISSION_DENIED.value() as u32,
        )
        .unwrap();
        let len2 = bb2.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::MONITOR,
                sequence: 0.into(),
                peer: 0.into(),
            },
            &buf2[..len2],
        );
    });

    let mut conn = Connection::new(client).unwrap();
    let mut frames = Vec::new();
    conn.monitor(|m| {
        frames.push((
            m.client,
            m.peer,
            m.send,
            m.seq,
            m.cmd_type,
            m.status,
            m.data.to_vec(),
        ));
        frames.len() < 2
    })
    .unwrap();

    assert_eq!(frames.len(), 2);
    assert_eq!(
        frames[0],
        (
            0x100,
            0x200,
            true,
            7,
            UbusCmdType::INVOKE,
            None,
            vec![1, 2, 3, 4],
        )
    );
    assert_eq!(
        frames[1],
        (
            0x200,
            0x100,
            false,
            7,
            UbusCmdType::STATUS,
            Some(UbusStatus::PERMISSION_DENIED),
            vec![],
        )
    );
    server_handle.join().unwrap();
}

#[test]
fn test_wait_for_existing_and_delayed() {
    #[derive(Serialize)]
    struct ObjAdd<'a> {
        id: i32,
        path: &'a str,
    }

    let (client, mut server) = UnixStream::pair().unwrap();
    let mock_ubusd = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();

        // 1. register_subscriber -> ADD_OBJECT(600)
        let (add_hdr, _) = read_msg(&mut server);
        assert_eq!(add_hdr.cmd_type, UbusCmdType::ADD_OBJECT);
        let mut add_reply = [0u8; 32];
        let mut ab = BlobBuilder::from_bytes(&mut add_reply);
        ab.push_u32(BlobAttrId::OBJID.value(), 600).unwrap();
        let alen = ab.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: add_hdr.sequence,
                peer: 0.into(),
            },
            &add_reply[..alen],
        );
        write_status(&mut server, add_hdr.sequence, 0, 0);

        // 2. event register("ubus.object.add")
        let (ev_reg_hdr, _) = read_msg(&mut server);
        assert_eq!(ev_reg_hdr.cmd_type, UbusCmdType::INVOKE);
        assert_eq!(u32::from(ev_reg_hdr.peer), UBUS_SYSTEM_OBJECT_EVENT);
        write_status(&mut server, ev_reg_hdr.sequence, UBUS_SYSTEM_OBJECT_EVENT, 0);

        // 3. lookup("late.svc") -> NOT_FOUND
        let (lk_hdr, _) = read_msg(&mut server);
        assert_eq!(lk_hdr.cmd_type, UbusCmdType::LOOKUP);
        write_status(&mut server, lk_hdr.sequence, 0, UbusStatus::NOT_FOUND.value());

        // 4. Send unrelated "other.svc" add event
        let ev1 = to_blob(&ObjAdd {
            id: 0x1111,
            path: "other.svc",
        })
        .unwrap();
        let mut inv1 = [0u8; 128];
        let mut ib1 = BlobBuilder::from_bytes(&mut inv1);
        ib1.push_u32(BlobAttrId::OBJID.value(), 600).unwrap();
        ib1.push_str(BlobAttrId::METHOD.value(), "ubus.object.add")
            .unwrap();
        ib1.push_slice(BlobAttrId::DATA.value(), &ev1).unwrap();
        let ilen1 = ib1.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::INVOKE,
                sequence: 10u16.into(),
                peer: 1.into(),
            },
            &inv1[..ilen1],
        );
        let (ack1, _) = read_msg(&mut server);
        assert_eq!(ack1.cmd_type, UbusCmdType::STATUS);

        // 5. Send target "late.svc" add event (id = 0x7777)
        let ev2 = to_blob(&ObjAdd {
            id: 0x7777,
            path: "late.svc",
        })
        .unwrap();
        let mut inv2 = [0u8; 128];
        let mut ib2 = BlobBuilder::from_bytes(&mut inv2);
        ib2.push_u32(BlobAttrId::OBJID.value(), 600).unwrap();
        ib2.push_str(BlobAttrId::METHOD.value(), "ubus.object.add")
            .unwrap();
        ib2.push_slice(BlobAttrId::DATA.value(), &ev2).unwrap();
        let ilen2 = ib2.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::INVOKE,
                sequence: 11u16.into(),
                peer: 1.into(),
            },
            &inv2[..ilen2],
        );
        let (ack2, _) = read_msg(&mut server);
        assert_eq!(ack2.cmd_type, UbusCmdType::STATUS);

        // 6. REMOVE_OBJECT(600)
        let (rem_hdr, _) = read_msg(&mut server);
        assert_eq!(rem_hdr.cmd_type, UbusCmdType::REMOVE_OBJECT);
        write_status(&mut server, rem_hdr.sequence, 0, 0);
    });

    let mut conn = Connection::new(client).unwrap();
    let id = conn
        .wait_for("late.svc", Some(Duration::from_millis(500)))
        .unwrap();
    assert_eq!(id, 0x7777);
    assert_eq!(conn.timeout(), None);

    mock_ubusd.join().unwrap();
}

#[test]
fn test_buffer_high_water_mark_reclamation() {
    let (client, mut server) = UnixStream::pair().unwrap();

    let mock_ubusd = std::thread::spawn(move || {
        server.write_all(TEST_HELLO).unwrap();

        // 1. Large fire-and-forget invoke_no_reply (~12 KiB)
        let (hdr1, body1) = read_msg(&mut server);
        assert_eq!(hdr1.cmd_type, UbusCmdType::INVOKE);
        assert!(body1.len() >= 12 * 1024);

        // 2. Large RPC invoke (~16 KiB reply + status)
        let (hdr2, _) = read_msg(&mut server);
        assert_eq!(hdr2.cmd_type, UbusCmdType::INVOKE);
        let big_blob = vec![b'B'; 16 * 1024];
        let mut rep_buf = vec![0u8; 16 * 1024 + 32];
        let mut rb = BlobBuilder::from_bytes(&mut rep_buf);
        rb.push_slice(BlobAttrId::DATA.value(), &big_blob).unwrap();
        let rlen = rb.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: hdr2.sequence,
                peer: 10.into(),
            },
            &rep_buf[..rlen],
        );
        write_status(&mut server, hdr2.sequence, 10, 0);

        // 3. Low-level next_message: 16 KiB frame followed by 2048-byte frame (total_len <= 4096)
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: 1u16.into(),
                peer: 10.into(),
            },
            &rep_buf[..rlen],
        );
        let small_blob = [0x5au8; 2032];
        let mut small_buf = [0u8; 2048];
        let mut sb = BlobBuilder::from_bytes(&mut small_buf);
        sb.push_slice(BlobAttrId::DATA.value(), &small_blob)
            .unwrap();
        let slen = sb.len();
        assert_eq!(UbusMsgHeader::SIZE + BlobTag::SIZE + slen, 2048);
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::DATA,
                sequence: 2u16.into(),
                peer: 10.into(),
            },
            &small_buf[..slen],
        );

        // 4. Server dispatch_one receiving ~16 KiB INVOKE request
        let big_req = vec![b'C'; 16 * 1024];
        let mut req_buf = vec![0u8; 16 * 1024 + 64];
        let mut qb = BlobBuilder::from_bytes(&mut req_buf);
        qb.push_u32(BlobAttrId::OBJID.value(), 42).unwrap();
        qb.push_str(BlobAttrId::METHOD.value(), "big_srv").unwrap();
        qb.push_slice(BlobAttrId::DATA.value(), &big_req).unwrap();
        let qlen = qb.len();
        write_msg(
            &mut server,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                cmd_type: UbusCmdType::INVOKE,
                sequence: 3u16.into(),
                peer: 0x100.into(),
            },
            &req_buf[..qlen],
        );
        let (srv_st_hdr, _) = read_msg(&mut server);
        assert_eq!(srv_st_hdr.cmd_type, UbusCmdType::STATUS);

        // 5. Burst of 12 out-of-band INVOKE frames before PING STATUS reply
        let (ping_hdr, _) = read_msg(&mut server);
        assert_eq!(ping_hdr.cmd_type, UbusCmdType::PING);
        let mut burst_buf = [0u8; 64];
        let mut bb = BlobBuilder::from_bytes(&mut burst_buf);
        bb.push_u32(BlobAttrId::OBJID.value(), 42).unwrap();
        bb.push_str(BlobAttrId::METHOD.value(), "burst").unwrap();
        bb.push_bool(BlobAttrId::NO_REPLY.value(), true).unwrap();
        let blen = bb.len();
        for i in 0..12u16 {
            write_msg(
                &mut server,
                UbusMsgHeader {
                    version: UbusMsgVersion::CURRENT,
                    cmd_type: UbusCmdType::INVOKE,
                    sequence: (100 + i).into(),
                    peer: 0x200.into(),
                },
                &burst_buf[..blen],
            );
        }
        write_status(&mut server, ping_hdr.sequence, 0, 0);
    });

    let mut conn = Connection::new(client).unwrap();

    // 1. Large send shrinks after send_frame
    let big_send = vec![b'A'; 12 * 1024];
    conn.invoke_no_reply(10, "big_send", &big_send).unwrap();
    assert!(conn.buffer_capacity() <= BUFFER_HIGH_WATER_MARK);

    // 2. Large invoke receive shrinks on finish_deadline
    let mut got_recv_len = 0;
    conn.invoke(10, "big_recv", &[], |bi| {
        got_recv_len = bi.as_bytes().len();
    })
    .unwrap();
    assert_eq!(got_recv_len, 16 * 1024);
    assert!(conn.buffer_capacity() <= BUFFER_HIGH_WATER_MARK);

    // 3. Low-level next_message grows for 16 KiB, then shrinks on 2048-byte frame (<= 4096)
    {
        let msg = conn.next_message().unwrap();
        assert!(msg.blob.data.len() >= 16 * 1024);
    }
    assert!(conn.buffer_capacity() >= 16 * 1024);
    {
        let msg = conn.next_message().unwrap();
        assert_eq!(msg.blob.data.len(), 2036);
    }
    assert!(conn.buffer_capacity() <= BUFFER_HIGH_WATER_MARK);

    // 4. Server dispatch_one shrinks after handling large request
    let mut srv_len = 0;
    let handled = conn
        .dispatch_one(|req| {
            srv_len = req.data.len();
            Ok(RequestReply::Status(UbusStatus::OK))
        })
        .unwrap();
    assert!(handled);
    assert_eq!(srv_len, 16 * 1024);
    assert!(conn.buffer_capacity() <= BUFFER_HIGH_WATER_MARK);

    // 5. Burst of 12 queued out-of-band messages drained via dispatch_pending
    conn.ping().unwrap();
    assert_eq!(conn.pending_count(), 12);
    let mut drained = 0usize;
    let count = conn
        .dispatch_pending(|ev| {
            if let IncomingEvent::Invoke(req) = ev {
                assert_eq!(req.method, "burst");
                drained += 1;
            }
            Ok(RequestReply::Status(UbusStatus::OK))
        })
        .unwrap();
    assert_eq!(count, 12);
    assert_eq!(drained, 12);
    assert_eq!(conn.pending_count(), 0);

    // 6. Explicit shrink_buffers releases all unused capacity
    conn.shrink_buffers();
    assert_eq!(conn.buffer_capacity(), 0);

    mock_ubusd.join().unwrap();
}
