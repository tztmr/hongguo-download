# 剧目数据来源与分页约定

核对日期：2026-09-09。本文说明项目实际使用的数据来源；上游页面参数不等同于已经验证的公开接口合同。

## 用户提供的 subscribe 链接

路径为 `/reading/user/subscribe/list/v:version/`，项目此前已用这个路径读取真人剧预约排期。返回的 `subscribe_items` 中，项目只保留 `is_online=true`、`content_type=1`，且 `schedule_publish_time` 属于目标上海日期的剧目。

| 项目 | 用户链接 | 当前项目 |
| --- | --- | --- |
| 主机 | `api5-normal-sinfonlinec.fqnovel.com` | `api5-normal-sinfonlineb.fqnovel.com` |
| 页面参数 | `bdhm_bid=novelread_lynx`、`single_tab=0` | 使用现有 app 请求画像 |
| 面板 | `active_panel=6` | `tab_type=5`、`active_panel=5` |
| 日期 | `target_date=20260905` | 由服务器按 `Asia/Shanghai` 生成当天日期 |
| 分页 | `offset=0`、空 `session_id` | 后续页沿用返回的 offset、session 与设备 |
| 其他筛选 | 空 `panels`、`version=0`、末尾空参数 `gender_` | `filter_type=gender`、`gender_type=2`、`order_experiment=descend` |

`20260905` 表示 2026-09-05，是本次核对日期之前的历史日期。项目的新剧监听接口当前不开放历史日期查询。

匿名、无 Cookie、无设备和签名的完整链接 GET 实测返回 **HTTP 200**，但业务返回 **`code=100103`、`message=ERR_PARAM_APPID_INVALID`**。因此 HTTP 成功不代表拿到了排期数据；该链接缺少当前项目请求使用的 `aid` 等 app 参数，不能单独作为可直接调用的完整请求。

尚未确认 `active_panel=6`、`single_tab`、`panels` 和尾部 `gender_` 的完整筛选语义，也未证实两个主机在所有场景完全等价。`gender_` 不能补猜成某个参数名。此次保持已验证的现有参数，未直接把面板改为 6，未由参数名称推断真人剧、漫剧或 AI 剧。

## 榜单与新剧监听

热度只来自上游的 `hot_score` 或明确标注“热度”的文本（包含 `rec_text`、`rec_text_item.RecommendText` 及副标题），不使用推荐次数、播放量代替。2026-09-11 排查发现 v0.3.0 旧安装包中的 API 未包含已提交的热度解析：同一首页响应在旧程序中缺少 `hot_count`，当前源码可解析出数值。已重建 API，并增加打包前源码哈希检查。新 DMG 的独立实测中，真人首页 6/6、漫剧首页 6/9、推荐榜 20/20、热播榜 19/20 条返回有效热度；上游未提供热度的剧目继续保留未知状态。

榜单来自 `/reading/bookapi/bookmall/cell/change/v:version/`。项目接口为：

- `GET /api/duanju/rank?board=...&type=...&cursor=...&limit=20`
- `GET /api/duanju/new-releases?type=...&cursor=...&limit=20`

新剧监听保留 `items`、`next_cursor`、`has_more`、`date`、`refreshed_at`，新增以下来源说明：

| type | source | date_scope | 含义 |
| --- | --- | --- | --- |
| `playlet` | `subscribe` | `today` | 按排期筛选北京时间今日上线的真人剧 |
| `comic_series_rank` | `rank` | `latest` | 漫剧新剧榜最新收录，可包含早于今天上线的剧目 |
| `ai_playlet` | `rank` | `latest` | AI 新剧榜最新收录，可包含早于今天上线的剧目 |

`date` 是这轮检查的上海日期，不表示 `latest` 列表中每部剧都在该日上线。前端使用 `dateScope` 对应后端 `date_scope`，按来源显示时间范围和通知文案。

分页与缓存：

- 每次返回最多 20 部，超取结果缓存在后端游标中；20 是响应页大小，不是总量上限。
- 游标绑定榜单/类型、上海日期及上游分页状态；当前有效期为 10 分钟。过期后从首屏刷新。
- 类型筛选失效而切回混合榜时，清空上游 offset、session、rank version 和首屏筛选状态，从混合榜首屏重启；保留本地去重，避免漏首屏或重复显示。
- 榜单上游页缓存 60 秒，按来源类型、榜单、日期、设备与完整分页状态隔离。同页并发请求合并；一个调用取消不会中断其他调用。请求错误和不能前进的分页结果不缓存。
- 新剧页缓存 60 秒，加入设备维度，避免不同设备复用同一分页会话。
- 漫剧/AI 剧批量补充指标失败时，保留已拿到的剧目及榜单已有信息；未知指标保持未知。
- 监听按页更新并保存已获取结果，完整翻页后才标记检查完成；后续页失败会保留结果并显示未完成状态。切换类型、停用或跨日后，旧请求不能写入当前列表。

## 官网分类

[官网分类页](https://hongguoduanju.com/category)的 SSR `_ROUTER_DATA.loaderData.category_page.selectorList` 提供筛选字典，项目只解析 JSON，不执行页面脚本。列表来自官网公开 JSON 路径 `/api/category/page`；适配器使用背景、主题、设定、受众、时间和推荐排序维度。

本次官网分类适配仅用于真人短剧（官网 `tab=1`）。官网“漫画”不等同于应用里的视频“漫剧”，不把官网漫画条目映射成可播放漫剧。榜单布局参考[官网热播榜](https://hongguoduanju.com/rank/hot-drama)，现有榜单与监听仍使用上述 app 数据来源。
