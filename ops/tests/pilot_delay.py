"""Delay each new connection's first live-file GET during the one-asset reproduction."""
from contextlib import contextmanager
import select
import socket
import socketserver
from threading import Event, Thread
from urllib.parse import urlsplit


@contextmanager
def delayed_reads(endpoint):
    origin = urlsplit(endpoint)
    assert origin.scheme == "http" and origin.hostname == "127.0.0.1"
    slow = Event()
    hits = Event()

    class Relay(socketserver.BaseRequestHandler):
        def handle(self):
            with socket.create_connection(("127.0.0.1", origin.port), timeout=5) as upstream:
                sockets = [self.request, upstream]
                try:
                    while True:
                        for sender in select.select(sockets, [], [], 15)[0]:
                            data = sender.recv(65536)
                            if not data:
                                return
                            if sender is self.request and slow.is_set() and data.startswith(b"GET /konae-test/files/"):
                                hits.set()
                                Event().wait(2.2)
                            receiver = upstream if sender is self.request else self.request
                            receiver.sendall(data)
                except OSError:
                    return

    class Server(socketserver.ThreadingTCPServer):
        daemon_threads = True

    with Server(("127.0.0.1", 0), Relay) as relay:
        thread = Thread(target=relay.serve_forever)
        thread.start()
        try:
            yield f"http://127.0.0.1:{relay.server_address[1]}", slow, hits
        finally:
            relay.shutdown()
            thread.join()
