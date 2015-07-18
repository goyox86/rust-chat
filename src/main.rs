extern crate mio;
extern crate http_muncher;
extern crate sha1;
extern crate rustc_serialize;

use mio::*;
use mio::tcp::*;
use std::collections::HashMap;
use http_muncher::{Parser, ParserHandler};
use rustc_serialize::base64::{ToBase64, STANDARD};
use std::cell::RefCell;
use std::rc::Rc;
use std::fmt;

fn gen_key(key: &String) -> String {
    let mut m = sha1::Sha1::new();
    let mut buf = [0u8; 20];

    m.update(key.as_bytes());
    m.update("258EAFA5-E914-47DA-95CA-C5AB0DC85B11".as_bytes());

    m.output(&mut buf);

    return buf.to_base64(STANDARD);
}

struct HttpParser {
    current_key: Option<String>,
    headers: Rc<RefCell<HashMap<String, String>>>
}

impl ParserHandler for HttpParser {
    fn on_header_field(&mut self, s: &[u8]) -> bool {
        self.current_key = Some(std::str::from_utf8(s).unwrap().to_string());
        true
    }

    fn on_header_value(&mut self, s: &[u8]) -> bool {
        self.headers.borrow_mut()
            .insert(self.current_key.clone().unwrap(),
                    std::str::from_utf8(s).unwrap().to_string());
        true
    }

    fn on_headers_complete(&mut self) -> bool {
        false
    }
}

struct WebSocketServer {
    socket: TcpListener,
    clients: HashMap<Token, WebSocketClient>,
    token_counter: usize,
}

const  SERVER_TOKEN: Token = Token(0);

impl Handler for WebSocketServer {
    type Timeout = usize;
    type Message = ();

    fn ready(&mut self, event_loop: &mut EventLoop<WebSocketServer>,
            token: Token, events: EventSet) {
        if events.is_readable() {
            match token {
                SERVER_TOKEN => {
                    let client_socket = match self.socket.accept() {
                        Err(e) => {
                            println!("Accept Error {}", e);
                            return;
                        },
                        Ok(None) => unreachable!(),
                        Ok(Some(sock)) => sock
                    };

                    let new_token = Token(self.token_counter);
                    self.clients.insert(new_token,  WebSocketClient::new(client_socket));
                    self.token_counter += 1;

                    event_loop.register_opt(&self.clients[&new_token].socket,
                                            new_token,
                                            EventSet::readable(),
                                            PollOpt::edge() | PollOpt::oneshot()).unwrap();
                },
                token =>  {
                    let mut client = self.clients.get_mut(&token).unwrap();
                    client.read();
                    event_loop.reregister(&client.socket, token, client.interest,
                                          PollOpt::edge() | PollOpt::oneshot()).unwrap();
                }
            }
        }

        if events.is_writable() {
            let mut client = self.clients.get_mut(&token).unwrap();
            client.write();
            event_loop.reregister(&client.socket, token, client.interest,
                                  PollOpt::edge() | PollOpt::oneshot()).unwrap();
        }
    }
}


#[derive(PartialEq)]
enum ClientState {
    AwaitingHandshake,
    HandshakeResponse,
    Connected
}

struct WebSocketClient {
    socket: TcpStream,
    http_parser: Parser<HttpParser>,
    headers: Rc<RefCell<HashMap<String, String>>>,
    interest: EventSet,
    state: ClientState
}

impl WebSocketClient {
    fn read(&mut self) {
        loop {
            let mut buf = [0; 2048];
            match self.socket.try_read(&mut buf) {
                Err(e) => {
                    println!("Error while reading socket: {:?}", e);
                    return;
                },
                Ok(None) => break,
                Ok(Some(len)) => {
                    self.http_parser.parse(&buf[0..len]);
                    if self.http_parser.is_upgrade() {
                        self.state = ClientState::HandshakeResponse;
                        self.interest.remove(EventSet::readable());
                        self.interest.insert(EventSet::writable());
                        break;
                    }
                }
            }
        }
    }

    fn write(&mut self) {
        // Get the headers HashMap from the Rc<RefCell<...>> wrapper:
        let headers = self.headers.borrow();
        // Find the header that interests us, and generate the key from its value:
        let response_key = gen_key(&headers.get("Sec-WebSocket-Key").unwrap());
        // We're using special function to format the string.
        // You can find analogies in many other languages, but in Rust it's performed
        // at the compile time with the power of macros. We'll discuss it in the next
        // part sometime.
        let response = fmt::format(format_args!("HTTP/1.1 101 Switching Protocols\r\n\
                                                 Connection: Upgrade\r\n\
                                                 Sec-WebSocket-Accept: {}\r\n\
                                                 Upgrade: websocket\r\n\r\n", response_key));

        // Write the response to the socket:
        self.socket.try_write(response.as_bytes()).unwrap();
        // Change the state:
        self.state = ClientState::Connected;
        // And change the interest back to `readable()`:
        self.interest.remove(EventSet::writable());
        self.interest.insert(EventSet::readable());
    }

    fn new(socket: TcpStream) -> WebSocketClient {
        let headers = Rc::new(RefCell::new(HashMap::new()));

        WebSocketClient {
            socket: socket,
            headers: headers.clone(),
            http_parser: Parser::request(HttpParser {
                current_key: None,
                headers: headers.clone()
            }),
            interest: EventSet::readable(),
            state: ClientState::AwaitingHandshake
        }
    }
}

use std::str::FromStr;

fn main() {
    let server_socket = TcpSocket::v4().unwrap();
    let address = FromStr::from_str("0.0.0.0:10000").unwrap();
    server_socket.bind(&address).unwrap();
    let server_socket = server_socket.listen(128).unwrap();

    let mut event_loop = EventLoop::new().unwrap();

    let mut server = WebSocketServer {
        token_counter: 1,        // Starting the token counter from 1
        clients: HashMap::new(), // Creating an empty HashMap
        socket: server_socket    // Handling the ownership of the socket to the struct
    };

    event_loop.register_opt(&server.socket,
                           SERVER_TOKEN,
                           EventSet::readable(),
                           PollOpt::edge()).unwrap();
    event_loop.run(&mut server).unwrap();
}
