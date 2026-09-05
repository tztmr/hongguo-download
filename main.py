import argparse
import os
from contextlib import asynccontextmanager


def _runtime_arguments():
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--health-probe", action="store_true")
    parser.add_argument("--host", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=8000)
    parser.add_argument("--data-dir")
    return parser.parse_args() if __name__ == "__main__" else None


_RUNTIME_ARGUMENTS = _runtime_arguments()
if _RUNTIME_ARGUMENTS is not None and _RUNTIME_ARGUMENTS.data_dir:
    os.environ["HONGGUO_DATA_DIR"] = _RUNTIME_ARGUMENTS.data_dir

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from core.http_client import PureSignedClient
from core.scheduler import scheduler
from endpoints import device, duanju, pseries

@asynccontextmanager
async def lifespan(app: FastAPI):

    app.state.client = PureSignedClient(timeout=15.0)

    await scheduler.start()
    print("=" * 60)
    print("番茄短剧纯算 API 服务已启动")
    print("签名方式: 70932 (x-gorgon + x-khronos + x-argus + x-ladon)")
    print("SO 依赖:  无 | 纯 Python 实现")
    print("设备池:   24 小时自动过期, 每天定时注册 10 台, 池空自动注册")
    print("短剧模块: 加密短剧 (搜索/详情/目录/播放/密钥/解密下载/发现)")
    print("          PSeries 无签名短剧 (详情/目录/播放地址)")
    print("=" * 60)
    print("\n设备池空时调用加密短剧接口会自动注册, 无需先调注册接口")
    print("API 文档: http://localhost:8000/docs\n")
    yield

    await scheduler.stop()
    await app.state.client.close()

app = FastAPI(
    title="番茄短剧纯算 API",
    description="完全纯算签名, 无需 SO 文件。加密短剧支持 CENC 解密, 另提供 PSeries 无签名短剧。",
    version="2.0.0",
    lifespan=lifespan,
)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=False,
    allow_methods=["*"],
    allow_headers=["*"],
    expose_headers=["Content-Disposition", "X-Duanju-Definition", "Content-Length"],
)

app.include_router(device.router, prefix="/api", tags=["设备管理"])
app.include_router(duanju.router, prefix="/api", tags=["短剧(签名 + 解密)"])
app.include_router(pseries.router, prefix="/api", tags=["短剧(PSeries 无签名)"])

@app.get("/")
async def root():

    return {
        "name": "番茄短剧纯算 API",
        "version": "2.0.0",
        "sign_method": "70932 pure Python",
        "headers": ["X-Gorgon", "X-Khronos", "x-argus", "x-ladon"],
        "endpoints": {
            "device_register": "/api/device/register?count=10",
            "device_list": "/api/device/list",
            "device_cleanup": "/api/device/cleanup",
            "docs": "/docs",
            "--- 加密短剧 ---": "",
            "duanju_search": "/api/duanju/search?key=关键词",
            "duanju_detail": "/api/duanju/detail?book_id=短剧series_id",
            "duanju_catalog": "/api/duanju/catalog?book_id=短剧series_id",
            "duanju_content": "/api/duanju/content?item_id=剧集item_id",
            "duanju_key": "/api/duanju/key?item_id=剧集item_id&definition=720p",
            "duanju_download": "/api/duanju/download?item_id=剧集item_id&definition=720p",
            "duanju_categories": "/api/duanju/discovery/categories",
            "duanju_discovery": "/api/duanju/discovery?content_type=drama|manju",
            "duanju_discovery_more": "/api/duanju/discovery/more?cell_id=..&offset=..&session_id=..&plan_id=..&filter_ids=..",
            "duanju_rank": "/api/duanju/rank?board=ranklist_hot_sc&limit=20",
            "duanju_new_releases": "/api/duanju/new-releases?type=playlet&limit=20",
            "--- PSeries 无签名短剧 ---": "",
            "pseries_detail": "/api/pseries/detail?pseries_id=7657524778131000344",
            "pseries_catalog": "/api/pseries/catalog?pseries_id=7657524778131000344",
            "pseries_content": "/api/pseries/content?pseries_id=7657524778131000344&episode_index=1",
        },
        "device_pool": {
            "ttl_seconds": 86400,
            "daily_register_count": 10,
            "auto_register_on_empty": True,
            "auto_failover": True,
        },
        "note": "加密短剧走设备池 + 70932 签名; PSeries 为公开接口, 不使用签名与设备池",
    }

@app.get("/health")
async def health():

    from core.device_pool import device_pool
    return {
        "status": "ok",
        "api_contract": "hongguo-desktop-v2",
        "pool_size": device_pool.pool_size(),
        "active_count": device_pool.active_count(),
    }

if __name__ == "__main__":
    if _RUNTIME_ARGUMENTS.health_probe:
        if not isinstance(app, FastAPI):
            raise SystemExit(1)
        raise SystemExit(0)

    import uvicorn
    uvicorn.run(app, host=_RUNTIME_ARGUMENTS.host, port=_RUNTIME_ARGUMENTS.port)
