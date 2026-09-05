# 番茄短剧 API 使用文档

服务地址默认 `http://127.0.0.1:8000`,以下示例使用:

```bash
BASE=http://127.0.0.1:8000
```

除 `/api/duanju/download` 返回 `video/mp4` 二进制外,所有接口统一返回:

```json
{"code": 0, "msg": "ok", "data": {}}
```

失败时 `code` 非 0,`data` 为 `null`。

---

## 一、加密短剧 API

使用 70932 纯算签名和设备池;视频为 CENC 加密,需要 `spade_a` 派生密钥后解密。

### 1. 搜索短剧

```bash
# 首页: passback 留空
curl --get "$BASE/api/duanju/search" \
  --data-urlencode "key=系统" \
  -d "offset=0" \
  -d "passback=" \
  -d "content_type=drama"

# 后续页: 使用上一页返回的 next_offset 和 next_passback
curl --get "$BASE/api/duanju/search" \
  --data-urlencode "key=系统" \
  -d "offset=10" \
  -d "passback=10" \
  -d "content_type=drama"
```

| 参数 | 必填 | 默认值 | 说明 |
|---|---:|---|---|
| `key` | 是 | - | 搜索关键词 |
| `offset` | 否 | `0` | 分页偏移,使用上一页返回的 `next_offset` |
| `passback` | 否 | 空 | 番茄原生分页状态,使用上一页返回的 `next_passback` |
| `content_type` | 否 | `drama` | `drama` 为短剧,`manju` 为漫剧 |

`data.items[]` 关键字段:`book_id`(用于详情和目录)、`first_vid`(可直接用于播放模型)、`title`、`cover`、`episode_count`、`score`、`abstract`。同时返回 `has_more`、`next_offset` 和 `raw` 上游响应。

#### 切换短剧与漫剧

只切换 `content_type`,其余参数保持一致:

```bash
# 短剧:默认值,可省略 content_type
curl --get "$BASE/api/duanju/search" --data-urlencode "key=系统" -d "content_type=drama"

# 漫剧
curl --get "$BASE/api/duanju/search" --data-urlencode "key=系统" -d "content_type=manju"
```

| `content_type` | 内容 | 上游 tab | 内部画像 | 预期 `content_type` |
|---|---|---:|---:|---:|
| `drama` | 真人短剧 | 11 | 66.9 | 1 |
| `manju` | AI 漫剧 | 19 | 70132 | 1004 |

搜索结果中的 `book_id`、`first_vid` 后续都使用同一套 `/api/duanju/detail`、`/api/duanju/catalog`、`/api/duanju/content`、`/api/duanju/download` 接口。无需手动传版本号或签名参数。

### 2. 详情

```bash
curl "$BASE/api/duanju/detail?book_id=7600384110636321816"
```

返回上游原始详情结构。

### 3. 剧集目录

```bash
curl "$BASE/api/duanju/catalog?book_id=7600384110636321816"
```

`data.items[]` 每项包含从 1 开始的 `index`、剧集 `item_id` 和 `title`。`item_id` 即播放接口所需 ID。

### 4. 播放模型(加密地址)

```bash
curl "$BASE/api/duanju/content?item_id=7600387588385410073"
```

`data.sources[]` 每个清晰度包含:

| 字段 | 说明 |
|---|---|
| `definition` | 清晰度,如 `1080p`、`720p` |
| `urls` | 加密 MP4 的主/备 CDN 地址 |
| `spade_a` | CENC 密钥派生参数 |
| `codec_type` | 编码,如 `bytevc1` |
| `width` / `height` / `duration` / `size` | 视频元数据 |

此接口返回的 MP4 是加密的,直接播放会失败。

### 5. 派生解密密钥

```bash
curl "$BASE/api/duanju/key?item_id=7600387588385410073&definition=720p"
```

| 参数 | 必填 | 默认值 | 说明 |
|---|---:|---|---|
| `item_id` | 是 | - | 剧集 ID |
| `definition` | 否 | `720p` | 目标清晰度 |

返回:

```json
{
  "code": 0,
  "msg": "ok",
  "data": {
    "item_id": "7600387588385410073",
    "definition": "720p",
    "key_hex": "32 位十六进制 AES key",
    "encryption_method": "cenc-aes-ctr",
    "urls": ["https://..."],
    "size": 9625532
  }
}
```

适合自行下载并在客户端解密的场景。

### 6. 下载并解密(推荐)

```bash
curl -o ep1.mp4 "$BASE/api/duanju/download?item_id=7600387588385410073&definition=720p"
```

服务端完成全部流程后返回可直接播放的 MP4:

```text
Content-Type: video/mp4
Content-Disposition: attachment; filename="{item_id}_{definition}.mp4"
X-Duanju-Definition: 实际使用的清晰度
```

`definition` 支持 `auto`、`1080p`、`720p`、`540p`、`480p`、`360p`。`auto` 优先 1080p，其次 720p，再按现有安全档位降级。固定目标档位缺失时先降档、再升档,最终回落到任一可用源;实际档位以响应头 `X-Duanju-Definition` 为准。

解密流程:

```text
multi_video_model → spade_a → AES key
  → 下载加密 MP4
  → 解析 stsz / stsc / stco 或 co64 / senc
  → 逐 sample AES-CTR 解密
  → 清理 senc/saio/saiz/sinf/tenc 等 box,encv/enca 还原为 avc1/mp4a
  → 输出明文 MP4
```

### 7. 发现页(剧场栏 bookmall 协议)

对齐番茄畅读 7.2.3.32 剧场栏「真人剧 / 漫剧」推荐流。两步式翻页:首屏走 `bookmall/tab/v`,后续页走 `bookmall/cell/change/v`,翻页状态(`cell_id` / `next_offset` / `session_id` / `plan_id` / `filter_ids`)由响应自动给出,调用方原样回传即可。

#### 首屏

```bash
# 真人剧
curl "$BASE/api/duanju/discovery?content_type=drama"

# 漫剧
curl "$BASE/api/duanju/discovery?content_type=manju"
```

| 参数 | 必填 | 默认值 | 说明 |
|---|---:|---|---|
| `content_type` | 否 | `drama` | `drama`=真人剧(tab_type=38),`manju`=漫剧(tab_type=32) |
| `filter_ids` | 否 | 空 | 换一换去重队列,传上次响应返回的 `filter_ids` 可整体刷新推荐 |
| `selected_items` | 否 | 空 | 分类筛选,如 `cate_20`。来自首屏 `categories[].id`。首屏仍可能返回推荐缓存,真正换一批剧需把该值原样传给 `/discovery/more` |

首屏额外返回:

- `categories[]`: `{id,name,group}`,来自 bookmall `cell_selector`
- `rank_boards[]`: `{label,schema}`,来自剧目 `rec_tags` 里的 `sslocal://mainRank`
- `items[].rank_tags[]`: 海报角标,如 `神豪榜 No.1`

首页分组横栏使用标准化端点：

```bash
curl "$BASE/api/duanju/categories?content_type=drama"
```

返回 `data.groups[]`，每组为 `{id,name,items:[{id,name}]}`，并保持上游“综合、时代背景、主题情节、角色设定”的原始顺序。

完整排行榜列表页不是独立 `/rank` 接口,复用 `bookmall/cell/change/v`,见下一节。

响应关键字段:

```json
{
  "code": 0,
  "data": {
    "tab_type": "38",
    "items": [{ "series_id": "7674534642409540632", "book_id": "7674534642409540632",
                "title": "饥荒年，我养媳妇又养村", "cover": "https://...",
                "first_vid": "7674537498881559577", "episode_count": 80,
                "duration": 61200, "abstract": "...", "score": "", "category": "", "author": "" }],
    "cell_id": "7641597426175836222",
    "next_offset": 6,
    "has_more": true,
    "session_id": "20260829...",
    "plan_id": "7641597620900610110",
    "filter_ids": "7674534642409540632,7675683462497438744,...",
    "tab_list": [{ "tab_type": 8, "title": "找剧" }, { "tab_type": 38, "title": "真人剧" }, ...]
  }
}
```

#### 翻页(第 2 页起)

```bash
curl "$BASE/api/duanju/discovery/more?content_type=drama" \
  --get \
  -d "cell_id=7641597426175836222" \
  -d "offset=6" \
  -d "session_id=20260829..." \
  -d "plan_id=7641597620900610110" \
  -d "filter_ids=7674534642409540632,7675683462497438744,..."
```

| 参数 | 必填 | 说明 |
|---|---:|---|
| `content_type` | 否 | 同首屏 |
| `cell_id` | 是 | 首屏返回的推荐 cell_id,翻页期间不变 |
| `offset` | 是 | **上一页响应的 `next_offset`**(6→12→18…),不是本地累加 |
| `session_id` | 是 | **上一页响应返回值,每页轮换,必须用最新值** |
| `plan_id` | 是 | 首屏返回的 `plan_id`(bookstore_id),翻页期间不变 |
| `filter_ids` | 是 | **上一页响应的 `filter_ids`(series_id 去重队列,上限 200 自动滚动淘汰)**,防重复关键 |

响应结构与首屏一致(不含 `tab_list`),并返回新的 `session_id` / `next_offset` / `filter_ids`,继续回传即可无限翻页,直到 `has_more=false`。

#### 翻页状态机(与番茄客户端一致)

```text
首屏 bookmall/tab/v (client_req_type=1)
  └─ 响应: items + cell_id + next_offset=6 + session_id + plan_id + filter_ids
翻页 bookmall/cell/change/v (client_req_type=2, change_type=0, unlimited_selector_change_type=1)
  ├─ offset = 上一页 next_offset
  ├─ session_id = 上一页响应值(每页轮换)
  ├─ filter_ids = 累积 series_id 队列(缺它第 2 页会大量重复)
  └─ 响应: items + 新 next_offset + 新 session_id + 累积 filter_ids + has_more
```

#### 完整调用链示例

```bash
BASE=http://127.0.0.1:8000

# 1. 发现页首屏(真人剧)
D=$(curl -s "$BASE/api/duanju/discovery?content_type=drama")
BOOK_ID=$(echo "$D" | python -c "import json,sys; print(json.load(sys.stdin)['data']['items'][0]['series_id'])")
CELL=$(echo "$D" | python -c "import json,sys; print(json.load(sys.stdin)['data']['cell_id'])")
OFF=$(echo "$D" | python -c "import json,sys; print(json.load(sys.stdin)['data']['next_offset'])")
SESS=$(echo "$D" | python -c "import json,sys; print(json.load(sys.stdin)['data']['session_id'])")
PLAN=$(echo "$D" | python -c "import json,sys; print(json.load(sys.stdin)['data']['plan_id'])")
FIDS=$(echo "$D" | python -c "import json,sys; print(json.load(sys.stdin)['data']['filter_ids'])")

# 2. 翻第 2 页(状态原样回传)
curl -s --get "$BASE/api/duanju/discovery/more" \
  -d "content_type=drama" -d "cell_id=$CELL" -d "offset=$OFF" \
  -d "session_id=$SESS" -d "plan_id=$PLAN" --data-urlencode "filter_ids=$FIDS"

# 3. 点开一部: 目录 → 播放 → 解密下载(与搜索结果同一套接口)
ITEM_ID=$(curl -s "$BASE/api/duanju/catalog?book_id=$BOOK_ID" \
  | python -c "import json,sys; print(json.load(sys.stdin)['data']['items'][0]['item_id'])")
curl -o ep1.mp4 "$BASE/api/duanju/download?item_id=$ITEM_ID&definition=720p"
```

#### 旧版分类发现页(兼容保留)

```bash
curl "$BASE/api/duanju/discovery/categories"   # 分类列表
```

旧 `/api/duanju/discovery?category_id=...` 落地页接口已由 bookmall 协议取代;`discovery/categories` 仍可用。

| 参数 | 必填 | 默认值 | 范围 |
|---|---:|---|---|
| `category_id` | 是 | - | - |
| `offset` | 否 | `0` | ≥ 0 |
| `limit` | 否 | `12` | 1–50 |
| `gender` | 否 | `0` | 0–2 |

### 8. 排行榜(抓包已复现)

来源:`番茄.reqable_collection123.json`。完整榜单页不是独立 `/rank` 接口,复用:

`GET /reading/bookapi/bookmall/cell/change/v:version/`

抓包固定值:

| 字段 | 值 | 说明 |
|---|---|---|
| host | `api5-normal-sinfonlineb.fqnovel.com` | 排行榜抓包使用的上游 |
| `aid` | `8662` | `app_name=novelread`, iOS `version_name=7.3.2.32` |
| `tab_type` | `26` | 排行榜专用 tab,不是发现页的 38/32 |
| `cell_id` | `7470092475068071998` | 排行榜 cell,抓包全程未变 |
| `client_req_type` | `2` | 首屏、翻页均固定 |
| `unlimited_selector_change_type` | `2` 切榜 / `1` 翻页 | |

本地接口固定上游 `selected_items=all`，切榜只传 `board`：

| `board` | 含义 |
|---|---|
| `ranklist_hot_sc` | 推荐榜 |
| `ranklist_hot_play_sc` | 热播榜 |
| `ranklist_prestige` | 臻果榜 |
| `ranklist_subscribe` | 预约榜 |
| `ranklist_new_rank_sc` | 新剧榜 |
| `ranklist_hot_search_sc` | 热搜榜 |
| `ranklist_must_watch` | 必看榜 |
| `ranklist_followed` | 收藏榜 |

上游成功响应含 `data.cell_view.cell_data[].video_data[]`，以及 `has_more` / `next_offset` / `session_id`。`cell_selector.outer_row` 是榜单 tab；本地接口已将这些上游字段收敛为标准剧目列表和不透明游标。

本地已封装:

```bash
curl "$BASE/api/duanju/rank?board=ranklist_hot_sc&limit=20"
curl --get "$BASE/api/duanju/rank" \
  -d "board=ranklist_hot_sc" -d "limit=20" --data-urlencode "cursor=..."
```

客户端只需原样回传 `next_cursor`。本地服务在服务端保存并校验榜单、日期、`offset`、`session_id`、`rank_version`、最近的去重 ID 和粘性设备；这些上游状态都不会返回客户端。只要上游仍有下一页，本地接口就继续生成游标，不设总页数或总条目上限。

同文件里另外两条:

| 接口 | 用途 |
|---|---|
| `GET /reading/bookapi/plan/v:version/?selected_items=all&sub_selected_items=ranklist_hot_sc` | 打开排行榜时的前置请求 |
| `GET /reading/bookapi/new_category/front/v:version/?new_category_tab=6&source=gold_banner` | 金刚位「筛选」分类首页,返回 `down_category[]` |
| `POST /novel/player/multi_video_detail/preload/v1/` | 榜单封面预加载,body 为 `series_id` 列表,不是榜单本身 |

### 9. 剧目上线时间与指标

```bash
curl --get "$BASE/api/duanju/series-metrics" \
  --data-urlencode "series_id=7600384110636321816" \
  -d "content_type=1004"
```

`content_type` 应传列表项返回的数字值；兼容默认值为 `1`。响应不包含上游原始数据，只包含：

| 字段 | 说明 |
|---|---|
| `online_time` | 上线时间 Unix 时间戳 |
| `play_count` | 播放量 |
| `hot_count` | 热度量 |
| `collect_count` | 收藏量 |
| `like_count` | 点赞量 |

数值 `0` 保留为零，上游缺失则返回 `null`。该请求继续使用现有纯算签名、设备池、自动注册和失败换设备链路。

### 10. 北京时间今日新剧

```bash
# 首组，每次传输最多 20 条
curl --get "$BASE/api/duanju/new-releases" -d "type=playlet" -d "limit=20"

# 下一组：cursor 使用上次返回值
curl --get "$BASE/api/duanju/new-releases" \
  -d "type=playlet" -d "limit=20" --data-urlencode "cursor=..."
```

`type` 只支持 `playlet`（真人剧）、`comic_series_rank`（漫剧）和 `ai_playlet`（AI剧）。服务端用 `Asia/Shanghai` 日历日过滤上线时间，当天不足 20 条时只返回实际数量，不使用历史剧补齐。

真人剧直接使用抓包“短剧信息”中的按日上线接口；漫剧和 AI 剧分别使用自己的新剧榜 selector，并按页批量补齐上线时间与指标。服务端持续扫描到凑满当前传输页或上游自然结束，不设扫描页数上限；若上游声称还有数据但分页状态不前进，会返回错误而不是死循环。

返回字段为 `items`、`next_cursor`、`has_more`、`date`和 `refreshed_at`。游标是有效期 10 分钟的本地短令牌；过期、服务重启、跨日或更换 `type` 后使用旧游标会返回 HTTP 400，客户端应从空游标重新加载。

桌面端一次刷新会沿游标读取到自然结束，并按“日期 + 三种类型”保存当天完整结果；详细分类由剧目标签归纳，包含校园、古风和其他上游标签，缺少标签的归入“其他”。首次成功刷新只建立当天基线，不通知已存在的剧；之后刷新才对新增 `series_id` 合并发送系统通知。应用保持运行时每 5 分钟刷新一次。

---

## 二、PSeries 无签名短剧 API

与加密短剧完全隔离:公开 GET 请求,不生成签名头,不使用设备池,返回地址无需解密。

```bash
# 详情和当前页剧集列表
curl "$BASE/api/pseries/detail?pseries_id=7657524778131000344"

# 标准化目录
curl "$BASE/api/pseries/catalog?pseries_id=7657524778131000344"

# 第 1 集播放地址
curl "$BASE/api/pseries/content?pseries_id=7657524778131000344&episode_index=1"
```

| 接口 | 参数 | 返回 |
|---|---|---|
| `GET /api/pseries/detail` | `pseries_id`,可选游标和 `count` | 剧集元数据、当前页全部剧集 |
| `GET /api/pseries/catalog` | `pseries_id`,可选游标和 `count` | 标准化剧集目录 |
| `GET /api/pseries/content` | `pseries_id`、`episode_index` | 单集无加密 MP4 URL 列表 |

公共参数:`min_behot_time`、`max_behot_time` 和 `count`(1–200,默认 200)。游标省略时使用 `pseries_id` 作为上游初始游标。

剧集字段:`index`、`episode_id`、`video_id`、`title`、`description`、`duration`、`definition`、`codec_type`、`size`、`urls`、`cover`。

---

## 三、设备管理

```bash
curl "$BASE/api/device/register?count=10"
curl "$BASE/api/device/list"
curl "$BASE/api/device/remove?device_id=xxx"
curl "$BASE/api/device/cleanup"
```

加密短剧接口在设备池为空时会自动注册,通常无需手动调用。PSeries 接口不使用设备池。

---

## 四、健康检查

```bash
curl "$BASE/health"
```

```json
{"status": "ok", "pool_size": 10, "active_count": 10}
```

---

## 五、完整调用链示例

```bash
BASE=http://127.0.0.1:8000

# 加密短剧:搜索 → 目录 → 解密下载
BOOK_ID=$(curl -s --get "$BASE/api/duanju/search" --data-urlencode "key=系统" \
  | python -c "import json,sys; print(json.load(sys.stdin)['data']['items'][0]['book_id'])")

ITEM_ID=$(curl -s "$BASE/api/duanju/catalog?book_id=$BOOK_ID" \
  | python -c "import json,sys; print(json.load(sys.stdin)['data']['items'][0]['item_id'])")

curl -o ep1.mp4 "$BASE/api/duanju/download?item_id=$ITEM_ID&definition=720p"

# PSeries:详情 → 播放地址
curl -s "$BASE/api/pseries/content?pseries_id=7657524778131000344&episode_index=1" \
  | python -c "import json,sys; print(json.load(sys.stdin)['data']['urls'][0])"
```

---

## 六、常见错误

| `code` | 含义 | 处理 |
|---:|---|---|
| `-3` | 上游请求失败或业务错误 | 重试;加密短剧会自动换设备 |
| `-4` | 无可用视频源或剧集序号越界 | 检查 `item_id` / `episode_index` |
| `-5` | `spade_a` 派生密钥失败 | 换清晰度或重新获取播放模型 |
| `-8` | 所有 CDN 下载失败 | CDN 地址有时效,重新请求播放模型 |
| `-9` | CENC 解密失败 | MP4 结构异常,换清晰度重试 |

---

## 七、上游画像说明

服务内部按接口自动选择上游 App 画像,调用方无需传参:

| 接口 | 画像 | 原因 |
|---|---|---|
| `duanju/search`(`drama`)、`detail`、`catalog` | `version_code=66.9` | 只返回 `content_type=1`,不混入漫剧 |
| `duanju/search`(`manju`) | `version_code=70132` | `tab_type=19` 仅在新画像下存在 |
| `duanju/discovery`、`discovery/more` | `aid=8662&novelread&72932` | 剧场栏 bookmall 协议专用画像(番茄畅读 7.2.3.32) |
| `duanju/content`、`key`、`download` | `aid=8662` | 视频播放模型独立画像 |
| `pseries/*` | 无签名 | 公开接口 |
