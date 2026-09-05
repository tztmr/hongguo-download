# 番茄短剧纯算 API 服务

使用**纯 Python 算法**生成签名头 (X-Gorgon, X-Khronos, x-ladon, x-argus),**无需任何 SO 文件**。仅保留短剧全套能力:加密短剧和 PSeries 无签名短剧。

## 特性

- 纯算签名:Simon cipher + SM3 + AES + 自定义 ARX 密码,全部 Python 实现
- **加密短剧 CENC 解密**:`spade_a` 派生 AES key,AES-CTR 解密 sample 并清理加密 box,直接输出可播放 MP4
- **PSeries 无签名短剧**:公开 GET 接口,不使用签名和设备池
- **设备池空时自动注册**:调用加密短剧接口时若池空,自动注册新设备
- **24 小时自动过期**:设备注册 24 小时后自动删除
- **每天定时注册**:凌晨 2:00 自动注册 10 台新设备
- **失败自动换设备重试**:加密短剧请求失败后自动更换设备或立即注册新设备重试
- 内存缓存:搜索/详情/目录/播放模型结果自动缓存

## 安装

```bash
cd fanqie_pure_api
pip install -r requirements.txt
```

## 启动

```bash
uvicorn main:app --host 0.0.0.0 --port 8000
```

启动后定时任务自动运行:
- 每天凌晨 2:00 注册 10 台新设备
- 每小时清理过期设备(超过 24 小时)

## 加密短剧接口

走 70932 签名和设备池,视频为 CENC 加密。

| 接口 | 功能 |
|---|---|
| `GET /api/duanju/search` | 搜索短剧,`content_type=drama` 或 `manju` |
| `GET /api/duanju/detail` | 获取短剧原始详情 |
| `GET /api/duanju/catalog` | 获取并标准化剧集目录 |
| `GET /api/duanju/content` | 获取播放模型:多清晰度加密 CDN 地址和 `spade_a` |
| `GET /api/duanju/key` | 派生 CENC AES 密钥,供外部播放器自行解密 |
| `GET /api/duanju/download` | 下载并解密,直接返回可播放 MP4 |
| `GET /api/duanju/discovery/categories` | 获取短剧发现页分类 |
| `GET /api/duanju/discovery` | 按分类获取短剧列表 |

```bash
# 搜索短剧(默认 content_type=drama)
curl --get "http://localhost:8000/api/duanju/search" \
  --data-urlencode "key=系统" \
  -d "content_type=drama"

# 搜索漫剧
curl --get "http://localhost:8000/api/duanju/search" \
  --data-urlencode "key=系统" \
  -d "content_type=manju"

# 详情和剧集目录
curl "http://localhost:8000/api/duanju/detail?book_id=7600384110636321816"
curl "http://localhost:8000/api/duanju/catalog?book_id=7600384110636321816"

# 播放模型(加密地址 + spade_a)
curl "http://localhost:8000/api/duanju/content?item_id=7600387588385410073"

# 仅取解密密钥
curl "http://localhost:8000/api/duanju/key?item_id=7600387588385410073&definition=720p"

# 下载并解密,输出明文 MP4
curl -o ep1.mp4 "http://localhost:8000/api/duanju/download?item_id=7600387588385410073&definition=720p"

# 发现页
curl "http://localhost:8000/api/duanju/discovery/categories"
curl "http://localhost:8000/api/duanju/discovery?category_id=262&offset=0&limit=12"
```

`definition` 支持 `1080p`、`720p`、`540p`、`480p`、`360p`。目标档位不存在时先降档再升档,最终回落到任一可用源。

上游画像差异(实测):短剧搜索、详情、目录走旧画像 `66.9`,只返回 `content_type=1`,不混入漫剧;漫剧搜索 `tab_type=19` 与发现页 `landing` 只在 `70132` 画像下可用,旧画像会返回 `SERVICE_ERROR`。这一切换已内置,调用方无需关心。

### 解密流程

```text
item_id
  → multi_video_model (70932 签名 POST)
  → 加密 CDN urls + spade_a
  → derive_key_from_spade_a() 派生 16 字节 AES key
  → 下载加密 MP4(CDN 无需签名)
  → 解析 moov/trak/stbl 的 stsz、stsc、stco/co64、senc
  → 逐 sample AES-CTR(64 位低位计数器)解密
  → 清理 senc/saio/saiz/sinf/tenc 等加密 box,encv/enca 还原为 avc1/mp4a
  → 输出可直接播放 MP4
```

## PSeries 无签名短剧接口

独立访问 `api5-normal-hl.toutiaoapi.com/ugc/video/v1/pseries`,仅使用公开 GET 参数;**不经过**签名,不使用设备池,也不影响加密短剧接口。

| 接口 | 功能 |
|---|---|
| `GET /api/pseries/detail` | 短剧详情 + 当前页完整剧集列表 |
| `GET /api/pseries/catalog` | 标准化剧集目录 |
| `GET /api/pseries/content` | 按集数返回无加密 MP4 地址 |

```bash
curl 'http://localhost:8000/api/pseries/detail?pseries_id=7657524778131000344'
curl 'http://localhost:8000/api/pseries/catalog?pseries_id=7657524778131000344'
curl 'http://localhost:8000/api/pseries/content?pseries_id=7657524778131000344&episode_index=1'
```

支持 `min_behot_time`、`max_behot_time` 和 `count`(1–200)跟随上游游标分页;游标缺省时使用 `pseries_id`。返回地址为原始 `video/mp4`,无需解密。

## 设备管理

```bash
curl "http://localhost:8000/api/device/register?count=10"
curl "http://localhost:8000/api/device/list"
curl "http://localhost:8000/api/device/cleanup"
```

## API 文档

启动后访问 `http://localhost:8000/docs` 查看完整 Swagger 文档。

## 签名算法

| 头 | 算法 |
|----|------|
| X-Gorgon | nibble swap + bit reverse + 固定密钥表 |
| X-Khronos | Unix 时间戳 |
| x-ladon | 自定义 ARX 分组密码 (34 轮, 64-bit 字, ECB) |
| x-argus | Simon cipher 128/256 (72 轮) + SM3 哈希 + AES-128-CBC + Protobuf |

## 设备池机制

- 设备池文件:`config/device_pool.json`
- **24 小时 TTL**:设备注册 24 小时后自动删除 (`DEVICE_TTL = 86400`)
- **每天定时注册**:凌晨 2:00 自动注册 10 台新设备
- **池空自动注册**:调用加密短剧接口时若池空,立即自动注册新设备
- **失败自动重试**:最多 3 次设备轮询 + 最后一次新设备重试;密钥相关错误立即注册新设备
- 最少使用优先调度
- 连续失败 3 次自动标记 failed,冷却 10 分钟后自动恢复
- PSeries 接口不参与设备池

## macOS 桌面应用

`desktop/` 提供下载队列、视频合并、可选 AI 音频/字幕处理、YouTube OAuth/可恢复上传和分类型系统通知。开发、隐私边界、AI 模型安装、Google Cloud 前置条件及发布验证说明见 [`desktop/README.md`](desktop/README.md)。

完整 ARM64 发布使用 `scripts/build-release.sh`，并由 `scripts/verify-release.sh` 对 `.app`/`.dmg` 做失败即停止的资源、架构、健康探针、许可证和敏感数据扫描。FFmpeg/ffprobe 与 AI 清单的真实发布地址和 SHA-256 未提供时，不应把现有包描述为完整、自包含的发布版本。第三方许可说明见 [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)。

## 目录结构

```text
fanqie_pure_api/
├── main.py                 # FastAPI 主程序 (启动调度器)
├── requirements.txt
├── config/
│   └── device_pool.json    # 设备池持久化
├── core/
│   ├── pure_sign.py        # 纯算签名入口
│   ├── sign_70932.py       # 签名实现 (Gorgon+Khronos+Ladon+Argus)
│   ├── device_pool.py      # 设备池管理 (24h TTL + 自动清理)
│   ├── device_register.py  # 设备注册 + registerkey
│   ├── http_client.py      # 签名 HTTP 客户端 (自动注册 + 设备轮询)
│   ├── scheduler.py        # 定时任务 (每日注册 + 每时清理)
│   ├── mp4_decrypt.py      # CENC MP4 解密 (spade_a 派生 + AES-CTR + box 清理)
│   ├── cache.py            # TTL 内存缓存
│   └── response.py         # 统一响应格式
└── endpoints/
    ├── device.py           # 设备管理 API
    ├── duanju.py           # 加密短剧 API (搜索/详情/目录/播放/密钥/解密下载/发现)
    └── pseries.py          # PSeries 无签名短剧 API
```
