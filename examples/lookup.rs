use std::{collections::HashMap, path::Path, str::FromStr};

use jsonpath_rust::JsonPathFinder;
use serde_json::{json, Value};
use std::net::Ipv4Addr;
use ubus::BlobMsgData;

fn main() {
    let socket = Path::new("/var/run/ubus/ubus.sock");

    let mut connection = match ubus::Connection::connect(&socket) {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("{}: Failed to open ubus socket. {}", socket.display(), err);
            return;
        }
    };
    connection
        .lookup(
            "system",
            |obj| {
                println!("\n{:?}", obj);
            },
            |sig| {
                println!("  {}({:?})", sig.name, sig.args);
            },
        )
        .unwrap();

    let obj_id = connection.lookup_id("network.interface.lan").unwrap();
    let argv = BlobMsgData::String("eth0");
    let args = HashMap::from([("name", argv)]);
    let args = BlobMsgData::Table(args);
    connection
        .invoke(obj_id, "status", None, |bi| {
            let mut json_output = "{\n".to_string();
            let mut first = true;
            for x in bi {
                if !first {
                    json_output += ",\n";
                }
                json_output += &format!("{}", x);
                first = false;
            }
            json_output += "\n}";
            let json: Value = serde_json::from_str(&json_output).unwrap();
            println!("{}", serde_json::to_string_pretty(&json).unwrap());
            //let path = JsonPath::parse("$.ipv4-address[*].address").unwrap();
            let finder =
                JsonPathFinder::from_str(&json_output, "$.ipv4-address[*].address").unwrap();
            for addr in finder.find_slice() {
                let addr = addr.to_data().to_string();
                println!("{}", addr);
            }
        })
        .unwrap();
    //println!("{:X}", obj_id);
}
