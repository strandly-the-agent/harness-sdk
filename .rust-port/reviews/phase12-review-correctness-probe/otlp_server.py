import http.server, sys, time
log = open(sys.argv[2], 'a')
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get('content-length', 0)); body = self.rfile.read(n)
        log.write(f"{time.time():.3f} POST {self.path} {n} bytes\n"); log.flush()
        self.send_response(200); self.send_header('content-type', 'application/x-protobuf'); self.send_header('content-length', '0'); self.end_headers()
    def log_message(self, *a): pass
http.server.HTTPServer(('127.0.0.1', int(sys.argv[1])), H).serve_forever()
