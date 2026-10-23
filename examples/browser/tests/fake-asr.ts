import { once } from 'node:events'
import { createServer } from 'node:http'
import { json } from 'node:stream/consumers'

export const FAKE_ASR_TEXT = 'これは偽の音声認識サービスの書き起こしです'

/// Fake of the vLLM endpoints the transcriber calls for Granite Speech 2b: `/health` and `/v1/chat/completions`.
export async function startFakeAsr(port: number) {
  const requests: { model: string; prompt: string }[] = []
  let healthyAt = 0
  const server = createServer(async (request, response) => {
    if (request.method === 'GET' && request.url === '/health') {
      response.statusCode = Date.now() < healthyAt ? 503 : 200
      response.end()
      return
    }
    if (request.method === 'POST' && request.url === '/v1/chat/completions') {
      const body = (await json(request)) as { model: string; messages: { content: { text: string }[] }[] }
      const [, prompt] = body.messages[0].content
      requests.push({ model: body.model, prompt: prompt.text })
      response.setHeader('content-type', 'application/json')
      response.end(JSON.stringify({ choices: [{ message: { role: 'assistant', content: FAKE_ASR_TEXT } }] }))
      return
    }
    response.statusCode = 404
    response.end()
  })
  await once(server.listen(port, '127.0.0.1'), 'listening')
  return {
    requests,
    /// Makes `/health` return 503 for the next `milliseconds`, like a GPU service scaled to zero.
    unhealthyFor(milliseconds: number) {
      healthyAt = Date.now() + milliseconds
    },
    close: () => server.close()
  }
}
