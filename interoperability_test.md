# 相互接続試験結果

|                              | CLIENT_SETUP | SERVER_SETUP | OBJECT | SUBSCRIBE | SUBSCRIBE_OK | SUBSCRIBE_ERROR | UNSUBSCRIBE | SUBSCRIBE_FIN | SUBSCRIBE_RST | ANNOUNCE | ANNOUNCE_OK | ANNOUNCE_ERROR | UNANNOUNCE | GOAWAY |
| ---------------------------- | ------------ | ------------ | ------ | --------- | ------------ | --------------- | ----------- | ------------- | ------------- | -------- | ----------- | -------------- | ---------- | ------ |
| SkyWay Client -> Meta Server | ✅           | -            | ❌     | ❌        | -            | -               | ❌          | ❌            | ❌            | ✅       | -           | -              | ❌         | ❌     |
| SkyWay Client <- Meta Server | -            | ✅           | ❌     | -         | ❌           | ❌              | -           | ❌            | ❌            | -        | ✅          | ❌             | -          | ❌     |
| Meta Client -> SkyWay Server | ✅           | -            | ❌     | ❌        | -            | -               | ❌          | ❌            | ❌            | ✅       | -           | -              | ❌         | ❌     |
| Meta Client <- SkyWay Server | -            | ✅           | ❌     | -         | ❌           | ❌              | -           | ❌            | ❌            | -        | ✅          | ❌             | -          | ❌     |
