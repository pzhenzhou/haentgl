## 1. how it differs from MySQL Test suite

The primary purpose of MTR is to test the compatibility of the Proxy with its proxy backend, including the command
results and the error codes of the response (if an error occurs). Unlike the MySQL tests, the tests must be run
externally. my_proxy is the proxy's customized test suite, most of which
are identical to MySQL's native cases.

## 2. How to run MTR tests.

### 2.1. Start the proxy and MySQL.

```shell
docker-compose --env-file ./docker/docker-compose.env  -f ./docker/docker-compose-with-mysqld.yml up -d
```

### 2.2. Login to the proxy container.

```shell
docker exec -it my_proxy /bin/bash
```

### 2.3 Run the test.

```shell
cd /usr/share/mysql/mysql-test
bash run-mono-proxy-test.bash root "" 3310
```

Test results and its output

```text
root@my_proxy:/usr/share/mysql/mysql-test# bash run-my-proxy-test.bash root "" 3310


Collecting tests...

==============================================================================

TEST                                      RESULT   TIME (ms) or COMMENT
--------------------------------------------------------------------------

worker[1] Using MTR_BUILD_THREAD 300, with reserved ports 16000..16019
worker[1] Creating var directory '/usr/share/mysql/mysql-test/var'...
worker[1] > result: MySQL, file_mode: 0
worker[1] mysql-test-run: WARNING: running this script as _root_ will cause some tests to be skipped
worker[1] Creating my.cnf file for extern server...
worker[1]  protocol= TCP
worker[1]  socket= /tmp/mysqld.sock
worker[1]  host= 127.0.0.1
worker[1]  password=
worker[1]  user= root
worker[1]  port= 3310
worker[1] > Running test: my_proxy.view
worker[1] > Setting timezone: DEFAULT
worker[1] > Started [mysqltest - pid: 36, winpid: 36]
worker[1] my_proxy.view                          worker[1] [ pass ]  44105
my_proxy.view                          [ pass ]  44105
worker[1] > Running test: my_proxy.connect
worker[1] > Setting timezone: DEFAULT
worker[1] > Started [mysqltest - pid: 38, winpid: 38]
worker[1] my_proxy.connect                       worker[1] [ pass ]    753
my_proxy.connect                       [ pass ]    753
worker[1] > Running test: my_proxy.insert_select
worker[1] > Setting timezone: DEFAULT
worker[1] > Started [mysqltest - pid: 40, winpid: 40]
worker[1] my_proxy.insert_select                 worker[1] [ pass ]   2072
my_proxy.insert_select                 [ pass ]   2072
worker[1] > Running test: my_proxy.insert_update
worker[1] > Setting timezone: DEFAULT
worker[1] > Started [mysqltest - pid: 42, winpid: 42]
worker[1] my_proxy.insert_update                 worker[1] [ pass ]   3965
my_proxy.insert_update                 [ pass ]   3965
worker[1] > Running test: my_proxy.select
worker[1] > Setting timezone: DEFAULT
worker[1] > Started [mysqltest - pid: 44, winpid: 44]
worker[1] my_proxy.select                        worker[1] [ pass ]  37400
my_proxy.select                        [ pass ]  37400
worker[1] > Running test: my_proxy.trigger
worker[1] > Setting timezone: DEFAULT
worker[1] > Started [mysqltest - pid: 46, winpid: 46]
worker[1] my_proxy.trigger                       worker[1] [ pass ]  15414
my_proxy.trigger                       [ pass ]  15414
worker[1] Server said BYE
worker[1] Stopping
```
