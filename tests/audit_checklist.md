# Audit Checklist

Use this as a practical audit sheet for this project. Status values mean:

- `PASS`: implemented and already covered by source code and/or automated tests.
- `MANUAL`: implemented, but should still be demonstrated live in browser/curl/devtools.
- `RECHECK`: implemented, but verification was flaky and should be repeated before the audit.
- `NO`: not implemented in this repository.

## Functional

| Audit point | What to show during audit | Project evidence | Status |
| --- | --- | --- | --- |
| How does an HTTP server work? | Explain: bind listener sockets, wait for readiness, accept clients, parse HTTP request, build response, write response, keep alive or close. | `src/main.rs`, `src/server.rs`, `src/http.rs`, `src/router.rs` | PASS |
| Which I/O multiplexing function is used and how does it work? | Say this server uses Linux `epoll`, not `select`. Show the sole epoll instance waiting for listener, client, and CGI-pipe readiness. | `src/epoll.rs:13-35`, `src/server.rs:94`, `123-171`, `README.md:84` | PASS |
| Is the server using only one select/equivalent to read requests and write answers? | Show the server creates one `Epoll`; listeners, clients, CGI stdin, and CGI stdout are all registered with it. There is no nested CGI wait loop. | `src/server.rs:94`, `134`, `229`, `280`, `569-625`; repository search for `Epoll::new` returns one call | PASS |
| Why is it important to use only one select and how was it achieved? | Explain one event loop avoids blocking per client and scales to many streams in one thread. Demonstrate that a static request completes while a CGI child sleeps. | `src/server.rs:123-171`, `628-678`, `tests/integration.rs:515-565` | PASS |
| Is there only one read or write per client per select/equivalent? | Show `handle_client_event` chooses one pending-response write or one request read, never both. The pipelining regression verifies responses are not overwritten. | `src/server.rs:304-333`, `335-362`, `510-567`, `tests/integration.rs:444-493` | PASS |
| Are return values for I/O functions checked properly? | Walk through `accept`, client/CGI `read` and `write`, `epoll_ctl`, and `epoll_wait`; show `WouldBlock` and `Interrupted` are handled separately. | `src/server.rs:260-272`, `345-360`, `528-566`, `628-661`, `src/cgi.rs:125-167`, `src/epoll.rs:19-35` | PASS |
| If an error is returned on a socket, is the client removed? | Show `close_client(fd)` on read/write/hup/epoll-modify errors; pending CGI jobs are cancelled and reaped with their client. | `src/server.rs:304-333`, `504-506`, `551-554`, `821-843`, `src/cgi.rs:195-204` | PASS |
| Is writing and reading always done through a select/equivalent? | All readiness-driven streams use the sole epoll: network sockets and non-blocking CGI pipes. Regular local files are not epoll-compatible on Linux and are ordinary finite filesystem operations, not client communication streams. | `src/server.rs:123-171`, `229`, `280`, `569-661`; `src/cgi.rs:89-97`, `125-167` | PASS |

## Configuration File

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| Single server with single port | Run `cargo run -- config/single-port.conf`, then `curl http://127.0.0.1:8080/`. | `config/single-port.conf` | PASS |
| Multiple servers with different ports | Run `cargo run -- config/multi-port.conf`, test `:8080` and `:8081`. | `config/multi-port.conf`, `tests/integration.rs:192-253` | PASS |
| Multiple servers with different hostnames on same IP:port | Use `curl --resolve a.test:PORT:127.0.0.1 http://a.test:PORT/` and `b.test`. | `config/virtual-hosts.conf`, `src/server.rs:462-481`, `tests/integration.rs:192-253` | PASS |
| Custom error pages | Hit a missing page and show custom 400/404/405/413 page body. | `config/default.conf:10-15`, `www/errors/`, `tests/integration.rs:102-107`, `333-338` | PASS |
| Client body limit | Run with `config/body-limit.conf`, POST body shorter and longer than 16 bytes. | `config/body-limit.conf`, `src/http.rs:169-183`, `src/router.rs:19-21`, `tests/integration.rs:184-189` | PASS |
| Routes are taken into account | Show `/`, `/uploads`, `/listing`, `/cgi`, `/old` behave differently. | `config/default.conf:17-47`, `src/router.rs:27-61`, `tests/integration.rs:83-188` | PASS |
| Default file when path is a directory | `GET /` and `GET /uploads/` should serve `index.html` when present. | `config/default.conf:20`, `27`, `src/router.rs:114-132` | PASS |
| Accepted methods per route | Try forbidden `DELETE /listing/alpha.txt` and allowed `DELETE /uploads/sample.bin`. | `config/default.conf:24-46`, `src/router.rs:31-35`, `76-78`, `165-191`, `tests/integration.rs:118-123`, `149-153` | PASS |

## Methods And Cookies

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| GET works properly | Test `/`, `/listing/`, `/missing`, `/old`. | `src/router.rs:80-132`, `tests/integration.rs:83-130` | PASS |
| POST works properly | Test raw upload POST and CGI POST. | `src/router.rs:134-163`, `tests/integration.rs:132-140`, `289-297`, `323-331` | PASS |
| DELETE works properly | Delete an uploaded file, verify `204`; delete forbidden target, verify `405` or `403`. | `src/router.rs:165-191`, `tests/integration.rs:149-153` | PASS |
| Wrong request does not crash server | Send malformed HTTP/1.1 without `Host`; then send valid request after it. | `src/http.rs:147-149`, `src/server.rs:329-337`, `tests/integration.rs:300-346` | PASS |
| Uploaded files can be fetched back without corruption | Upload binary bytes and compare downloaded body exactly. | `src/router.rs:292-330`, `tests/integration.rs:132-147` | PASS |
| Session and cookies system is present | Hit `/session` twice, second request reuses cookie and increments `X-Session-Visits`. | `src/server.rs:21`, `339-358`, `483-518`, `tests/integration.rs:155-182` | PASS |

## Browser Interaction

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| Browser connects with no issue | Open `http://127.0.0.1:8080/` with `config/default.conf`. | `tests/browser_check.md:3-10` | MANUAL |
| Request and response headers are correct | In devtools Network tab, inspect `HTTP/1.1`, `Content-Length`, `Content-Type`, `Location`, cookies. | `src/http.rs:372-447`, `tests/browser_check.md:12-17` | MANUAL |
| Wrong URL handled properly | Visit `/missing`; confirm custom 404 page and correct status. | `www/errors/404.html`, `tests/integration.rs:102-107` | MANUAL |
| Directory listing handled properly | Visit `/listing/`; confirm generated listing page. | `src/router.rs:252-290`, `tests/integration.rs:109-116` | MANUAL |
| Redirect handled properly | Visit `/old`; confirm `301` and `Location: /`. | `config/default.conf:44-47`, `src/router.rs:37-50`, `tests/integration.rs:125-130` | MANUAL |
| CGI works with chunked and unchunked data | Test `/cgi/echo.py` with both `Transfer-Encoding: chunked` and `Content-Length`; show slow CGI does not stall unrelated clients. | `src/cgi.rs`, `src/http.rs:158-167`, `tests/integration.rs:255-345`, `515-565` | PASS |

## Port Issues

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| Multiple ports and websites work | Run a config with shared and separate ports, then hit each host/port pair. | `tests/integration.rs:192-253` | PASS |
| Same port configured multiple times in one server should be caught | Run `cargo run -- config/duplicate-port.conf`; the duplicated server block should be discarded with warning while other valid server remains usable. | `config/duplicate-port.conf`, `src/config.rs:69-81`, `tests/integration.rs:255-298` | PASS |
| Multiple servers with common ports, one bad config should not kill valid ones | Show mixed config: invalid duplicated-port server is ignored, valid server still serves requests. | `src/config.rs:54-115`, `src/server.rs:127-203`, `tests/integration.rs:255-298` | PASS |

## Siege And Stress

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| `siege -b [IP]:[PORT]` availability at least 99.5% | The post-fix 25-concurrent empty-page run completed 133,907 transactions at 100.00% availability with zero failures. | `tests/stress.sh` | PASS |
| No memory leak | The RSS fallback completed 3,000 requests with zero failures and zero active connections; final RSS was 3,324 KB. `valgrind` was unavailable, so that stronger optional check remains recommended. | `tests/leak_check.sh`, `/tmp/opencode/localhost-leaks/rss-smoke.log`, `tests/leaks.md` | PASS |
| No hanging connection | Show request and CGI timeout handling, pending-CGI cancellation on disconnect, and stress behavior; check slow partial requests close with `408`. | `src/server.rs:664-715`, `778-843`, `src/cgi.rs:169-204`, `tests/integration.rs:346-394`, `443-565`, `tests/stress.sh` | PASS |

## Bonus

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| More than one CGI system such as Python/C++/Perl | Current default config only ships Python CGI. You would need more `cgi` mappings and scripts to claim this. | `config/default.conf:38-41`, `www/cgi/echo.py` | NO |
| Second implementation in another language | No second implementation exists in this repository. | Repository structure | NO |

## Verified Regressions

| Item | Notes | Status |
| --- | --- | --- |
| HTTP/1.1 keep-alive verification | `supports_http11_keep_alive_on_same_connection` passed in both debug and release suites after the CGI event-loop refactor. | PASS |
| One client I/O operation per event | `supports_pipelined_requests_without_overwriting_responses` verifies two pipelined requests produce two complete ordered responses. | PASS |
| Slow CGI concurrency | `slow_cgi_does_not_block_other_clients` starts a sleeping CGI request and verifies an unrelated static response arrives in under 700 ms. | PASS |

## Fast Demo Commands

```sh
cargo run -- config/default.conf
curl -i http://127.0.0.1:8080/
curl -i http://127.0.0.1:8080/missing
curl -i http://127.0.0.1:8080/listing/
curl -i http://127.0.0.1:8080/old
curl -i -X POST -H "Content-Type: text/plain" --data "hello" http://127.0.0.1:8080/cgi/echo.py
curl -i -X POST -H "Content-Type: text/plain" --data "01234567890123456789" http://127.0.0.1:8080/uploads
curl -i --resolve alpha.test:8080:127.0.0.1 http://alpha.test:8080/
curl -i --resolve beta.test:8080:127.0.0.1 http://beta.test:8080/
tests/stress.sh
```
