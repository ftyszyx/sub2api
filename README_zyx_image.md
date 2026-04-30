# Sub2API 图片接口调用说明

这份文档专门整理当前项目里与图片生成、图片编辑、流式图片返回相关的接口调用方式，便于直接对接和排障。

适用接口：

- `POST /v1/images/generations`
- `POST /v1/images/edits`
- 也支持不带 `/v1` 的别名：
  - `POST /images/generations`
  - `POST /images/edits`

项目路由入口见：

- [backend/internal/server/routes/gateway.go](backend/internal/server/routes/gateway.go)

## 1. 使用前提

图片接口只有在 API Key 绑定的分组平台是 `openai` 时才可用；如果分组不是 `openai`，接口会返回：

```json
{
  "error": {
    "type": "not_found_error",
    "message": "Images API is not supported for this platform"
  }
}
```

支持的账号类型：

- `API Key`
- `OAuth`

项目内能力判断见：

- [backend/internal/service/account.go](backend/internal/service/account.go)

## 2. 支持的图片模型

当前图片接口要求模型名必须是 `gpt-image-*` 这一类；默认模型是 `gpt-image-2`。

实现见：

- [backend/internal/service/openai_images.go](backend/internal/service/openai_images.go)

默认值：

```text
model = gpt-image-2
n = 1
```

如果不传 `model`，会自动补成 `gpt-image-2`。

## 3. 生图接口

### 3.1 基本请求

```bash
curl https://your-domain.example/v1/images/generations \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-image-2",
    "prompt": "a cute orange cat astronaut",
    "response_format": "b64_json"
  }'
```

典型返回：

```json
{
  "created": 1710000000,
  "data": [
    {
      "b64_json": "...",
      "revised_prompt": "a cute orange cat astronaut"
    }
  ]
}
```

### 3.2 常用字段

JSON 请求常用字段：

- `model`
- `prompt`
- `stream`
- `n`
- `size`
- `response_format`
- `quality`
- `background`
- `output_format`
- `moderation`
- `style`
- `output_compression`
- `partial_images`

其中：

- `response_format` 常见为 `b64_json` 或 `url`
- `stream: true` 表示请求流式图片返回
- `partial_images` 主要用于流式部分图片事件

字段解析见：

- [backend/internal/service/openai_images.go](backend/internal/service/openai_images.go)

## 4. 编辑图接口

### 4.1 multipart 方式上传图片

编辑图最常见的调用方式是 multipart：

```bash
curl https://your-domain.example/v1/images/edits \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -F "model=gpt-image-2" \
  -F "prompt=turn it into a cyberpunk night scene" \
  -F "image=@input.png;type=image/png"
```

如果要带遮罩：

```bash
curl https://your-domain.example/v1/images/edits \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -F "model=gpt-image-2" \
  -F "prompt=replace the background with aurora" \
  -F "image=@input.png;type=image/png" \
  -F "mask=@mask.png;type=image/png"
```

### 4.2 JSON 方式传图片 URL

编辑图也支持 JSON 传 `image_url`：

```bash
curl https://your-domain.example/v1/images/edits \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-image-2",
    "prompt": "replace the background with aurora",
    "images": [
      { "image_url": "https://example.com/source.png" }
    ],
    "mask": {
      "image_url": "https://example.com/mask.png"
    },
    "response_format": "b64_json"
  }'
```

注意限制：

- `images[].file_id` 不支持
- `mask.file_id` 不支持
- JSON 编辑图时必须提供 `images[].image_url`
- multipart 编辑图时必须提供至少一个 `image` 文件

相关校验见：

- [backend/internal/service/openai_images.go](backend/internal/service/openai_images.go)

## 5. 流式图片返回

这个项目支持图片流式返回。

### 5.1 生图流式请求

```bash
curl -N https://your-domain.example/v1/images/generations \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-image-2",
    "prompt": "a red panda on a bicycle",
    "stream": true,
    "response_format": "b64_json"
  }'
```

### 5.2 编辑图流式请求

```bash
curl -N https://your-domain.example/v1/images/edits \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -F "model=gpt-image-2" \
  -F "prompt=turn this into a neon rainy night market" \
  -F "stream=true" \
  -F "response_format=url" \
  -F "image=@input.png;type=image/png"
```

### 5.3 流式事件格式

项目对外返回的是 `text/event-stream`。

事件名：

- 生图中间结果：`image_generation.partial_image`
- 生图完成结果：`image_generation.completed`
- 编辑图中间结果：`image_edit.partial_image`
- 编辑图完成结果：`image_edit.completed`

事件名生成逻辑见：

- [backend/internal/service/openai_images_responses.go](backend/internal/service/openai_images_responses.go)

一个典型的 SSE 片段会像这样：

```text
event: image_generation.partial_image
data: {"type":"image_generation.partial_image","created_at":1710000001,"partial_image_index":0,"b64_json":"cGFydGlhbA==","model":"gpt-image-2"}

event: image_generation.completed
data: {"type":"image_generation.completed","created_at":1710000001,"b64_json":"ZmluYWw=","model":"gpt-image-2","usage":{"input_tokens":5,"output_tokens":9}}
```

说明：

- `partial_image` 里通常有 `partial_image_index` 和 `b64_json`
- `completed` 里通常有最终图片的 `b64_json`
- 如果请求里写了 `"response_format": "url"`，返回中还会带一个 `data:` URL

实现见：

- [backend/internal/service/openai_images_responses.go](backend/internal/service/openai_images_responses.go)

## 6. 前端接收流式图片示例

浏览器里如果要接收 SSE 风格的 POST 流，通常要用 `fetch` + `ReadableStream`，而不是 `EventSource`。

示例：

```ts
const resp = await fetch('https://your-domain.example/v1/images/generations', {
  method: 'POST',
  headers: {
    'Authorization': 'Bearer YOUR_API_KEY',
    'Content-Type': 'application/json'
  },
  body: JSON.stringify({
    model: 'gpt-image-2',
    prompt: 'a red panda on a bicycle',
    stream: true,
    response_format: 'b64_json'
  })
})

if (!resp.ok || !resp.body) {
  throw new Error(`HTTP ${resp.status}`)
}

const reader = resp.body.getReader()
const decoder = new TextDecoder()
let buffer = ''

while (true) {
  const { value, done } = await reader.read()
  if (done) break
  buffer += decoder.decode(value, { stream: true })

  let idx
  while ((idx = buffer.indexOf('\n\n')) >= 0) {
    const chunk = buffer.slice(0, idx)
    buffer = buffer.slice(idx + 2)

    const eventLine = chunk.split('\n').find(line => line.startsWith('event: '))
    const dataLine = chunk.split('\n').find(line => line.startsWith('data: '))
    if (!dataLine) continue

    const eventName = eventLine?.slice(7).trim() || ''
    const payload = JSON.parse(dataLine.slice(6))

    if (eventName === 'image_generation.partial_image') {
      console.log('partial image', payload)
    }

    if (eventName === 'image_generation.completed') {
      console.log('final image', payload)
    }
  }
}
```

## 7. CORS 注意事项

如果前端页面和 API 不同源，例如：

- 页面：`http://127.0.0.1:4321`
- API：`https://sub2api.1postpro.com`

那就需要在服务端配置允许跨域，否则浏览器会卡在预检请求。

配置示例：

```yaml
cors:
  allowed_origins:
    - "http://127.0.0.1:4321"
    - "http://localhost:4321"
  allow_credentials: true
```

相关文档可同时参考：

- [README_zyx.md](README_zyx.md)

## 8. 常见问题

### 8.1 为什么编辑图返回 502 / 524 / context canceled

常见原因：

- 上游图片生成耗时太长，被浏览器、Cloudflare 或其他代理提前断开
- 前端请求超时或主动 abort
- 输入图异常、太小、格式不规范

建议：

- 优先使用正常尺寸的 PNG / JPG
- 排查浏览器 Network 里的真实耗时和状态码
- 如果是浏览器场景，注意不要让前端 30 秒超时提前取消请求

### 8.2 为什么流式没有收到 partial_image

可能原因：

- 当前实际走的是非流式路径
- 上游模型/账号没有产出中间 partial image 事件
- 中间代理把长连接断开了

### 8.3 为什么接口提示不支持图片

检查：

- API Key 对应分组平台是否为 `openai`
- 账号类型是否为 `API Key` 或 `OAuth`
- 请求模型是否是 `gpt-image-*`

## 9. 代码参考

主要实现文件：

- [backend/internal/server/routes/gateway.go](backend/internal/server/routes/gateway.go)
- [backend/internal/handler/openai_images.go](backend/internal/handler/openai_images.go)
- [backend/internal/service/openai_images.go](backend/internal/service/openai_images.go)
- [backend/internal/service/openai_images_responses.go](backend/internal/service/openai_images_responses.go)
- [backend/internal/service/openai_images_test.go](backend/internal/service/openai_images_test.go)
