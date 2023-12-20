use std::{path::Path, convert::TryInto};

use ubus::{BlobMsgData, BlobMsg};

use std::net::Ipv4Addr;

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
                print!("  {}(", sig.name);
                for (name, ty) in sig.args {
                    print!("{}: {:?}, ", name, ty);
                }
                std::println!(")");
            },
        )
        .unwrap();

    let obj_id = connection.lookup_id("network.interface.lan").unwrap();
    let mut addressv4 = Ipv4Addr::UNSPECIFIED;
    
    connection.invoke(obj_id, "status", &[], |bi| {
        for x in bi {
            if x.name == Some("ipv4-address"){
                //println!("{:?}: {:?}", x.name.unwrap(), x.data);
                match x.data {
                    BlobMsgData::Array(addr_list) => {
                        for addr in addr_list {
                            if let BlobMsgData::Table(addr_table) = addr.data {
                                for in_addr in addr_table {
                                    match in_addr.name {
                                        Some("address") => if let BlobMsgData::String(addr_str) = in_addr.data {
                                            addressv4 = addr_str.parse().unwrap();
                                        },
                                        //Some("mask") => println!("mask: {:?}", in_addr.data),
                                        None => (),
                                        _ => (),
                                    }
                                }
                            }
                        }
                    },
                    _ => ()
                }
            }
        }
    }).unwrap();
    println!("{:X}", obj_id);
    println!("{:?}", addressv4);
}
