use std::path::Path;

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
    match connection.invoke(0x2770adca, "board", &[], |bi| {
        for x in bi {
            // let xs = match x.data {
            //     //BlobMsgData::Table(_) => todo!(),
            //     BlobMsgData::Int64(_) => todo!(),
            //     BlobMsgData::Int32(_) => todo!(),
            //     BlobMsgData::Int16(_) => todo!(),
            //     BlobMsgData::Int8(_) => todo!(),
            //     BlobMsgData::Double(_) => todo!(),
            //     BlobMsgData::Unknown(_, _) => todo!(),
            //     _ => 
            // }
            println!("{:?}: {:?}", x.name.unwrap_or_default(), x.data);
        }
    }) {
        Ok(res) => res,
        Err(err) => {
            eprintln!("Failed to invoke method. {}", err);
            return;
        }
    };
}
