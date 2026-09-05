from fastapi import APIRouter, Query, Request

from core.response import success, error
from core.device_pool import device_pool
from core.scheduler import scheduler

router = APIRouter()

@router.get("/device/register")
async def device_register(request: Request):

    count = int(request.query_params.get("count", "1"))
    result = await scheduler.register_now(count)
    return success(result)

@router.get("/device/list")
async def device_list():

    return success({
        "devices": device_pool.list_devices(),
        "pool_size": device_pool.pool_size(),
        "active_count": device_pool.active_count(),
    })

@router.get("/device/remove")
async def device_remove(request: Request):

    device_id = request.query_params.get("device_id", "")
    if not device_id:
        return error("缺少参数: device_id", code=-1)

    if device_pool.remove_device(device_id):
        return success({"device_id": device_id, "removed": True})
    return error("设备不存在", code=-1, status_code=404)

@router.get("/device/cleanup")
async def device_cleanup():

    before = device_pool.pool_size()
    device_pool.cleanup_expired()
    after = device_pool.pool_size()
    return success({
        "before": before,
        "after": after,
        "removed": before - after,
    })
