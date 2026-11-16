import http.server, sys
PAGES = {
 '/html': ('text/html; charset=utf-8', b'<html><head><title>T</title><style>p{}</style><script>var x=1;</script></head><body><h1>Head &amp; more</h1><p>Para   one\twith  spaces</p>\n\n\n\n<p>Two &lt;tag&gt; &nbsp; &#169; &quot;q&quot;</p><a href="/x">link</a><br/><ul><li>a</li><li>b</li></ul></body></html>'),
 '/text': ('text/plain', b'plain text body\nline 2\n'),
 '/json': ('application/json', b'{"a": [1, 2, {"b": null}]}'),
 '/latin1': ('text/html; charset=iso-8859-1', b'<p>caf\xe9 d\xe9j\xe0</p>'),
 '/nocharset': ('text/html', b'<p>caf\xc3\xa9 \xff bad</p>'),
 '/big': ('text/plain', (b'0123456789' * 20000)),
 '/bighuge': ('text/plain', (b'abcdefghij' * 1200000)),
 '/emoji': ('text/plain; charset=utf-8', ('x' * 49999 + '\U0001F600' + 'tail').encode()),
 '/bin': ('application/octet-stream', bytes(range(256))),
 '/empty': ('text/html', b''),
 '/md': ('text/markdown', b'# Title\n\nSome *md* text\n'),
 '/xml': ('application/xml', b'<?xml version="1.0"?><root><a>1</a></root>'),
 '/script': ('text/html', b'<SCRIPT type="x">alert(1)</SCRIPT><style>\n.a{}\n</style><div>keep</div><!-- comment -->after'),
 '/nested': ('text/html', b'<div><p>a</p>   <p>b</p>\n \n \n<pre>  pre   text\n  two</pre></div>'),
}
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        p = self.path.split('?')[0]
        if p == '/redir':
            self.send_response(302); self.send_header('location', '/text'); self.end_headers(); return
        if p == '/loop':
            self.send_response(302); self.send_header('location', '/loop'); self.end_headers(); return
        if p == '/404':
            self.send_response(404); self.send_header('content-type', 'text/plain'); self.end_headers(); self.wfile.write(b'nope'); return
        if p == '/500':
            self.send_response(500); self.end_headers(); return
        if p == '/ua':
            body = (self.headers.get('user-agent', '') + '|' + self.headers.get('accept', '')).encode()
            self.send_response(200); self.send_header('content-type', 'text/plain'); self.send_header('content-length', str(len(body))); self.end_headers(); self.wfile.write(body); return
        if p in PAGES:
            ct, body = PAGES[p]
            self.send_response(200); self.send_header('content-type', ct); self.send_header('content-length', str(len(body))); self.end_headers(); self.wfile.write(body); return
        self.send_response(404); self.end_headers()
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(('127.0.0.1', int(sys.argv[1])), H).serve_forever()
