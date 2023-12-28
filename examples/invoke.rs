use std::{collections::HashMap, env, path::Path, convert::TryInto};

use ubus::{BlobMsg, BlobMsgPayload};

fn main() {
    let argv: Vec<String> = env::args().collect();
    let mut obj_path = "";
    let mut method = "";
    //let mut args:Option<&BlobMsgPayload> = None;
    if argv.len() > 2 {
        obj_path = argv[1].as_str();
        method = argv[2].as_str();
    }

    let socket = Path::new("/var/run/ubus/ubus.sock");

    let mut connection = match ubus::Connection::connect(&socket) {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("{}: Failed to open ubus socket. {}", socket.display(), err);
            return;
        }
    };
    let obj_id = connection.lookup_id(obj_path).unwrap();
    let args = vec![BlobMsg{name: "name", data:BlobMsgPayload::String("eth0")}];
    connection
        .invoke(obj_id, method, args, |bi| {
            let mut json_output = "{\n".to_string();
            let mut first = true;
            for x in bi {
                if !first {
                    json_output += ",\n";
                }
                //json_output += &format!("{:?}", x);
                let msg: BlobMsg = x.try_into().unwrap();
                json_output += &format!("{}", msg);
                first = false;
            }
            json_output += "\n}";
/*             let json: Value = serde_json::from_str(&json_output).unwrap();
            println!("{}", serde_json::to_string_pretty(&json).unwrap()); */
            println!("{}", json_output);
            //let path = JsonPath::parse("$.ipv4-address[*].address").unwrap();
            /*             let finder =
                JsonPathFinder::from_str(&json_output, "$.ipv4-address[*].address").unwrap();
            for addr in finder.find_slice() {
                let addr = addr.to_data().to_string();
                println!("{}", addr);
            } */
        })
        .unwrap();
}
