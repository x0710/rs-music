globalThis.window = globalThis
globalThis.console = {
  log() {}, info() {}, warn() {}, error() {}, debug() {},
  group() {}, groupCollapsed() {}, groupEnd() {}
}

const lxHost = (() => {
  let initialized
  let handler
  const ready = new Promise(resolve => { initialized = resolve })
  const info = JSON.parse(__scriptInfo)
  const timers = new Map()
  let nextTimer = 1
  globalThis.setTimeout = (callback, delay = 0, ...args) => {
    if (typeof callback !== 'function') throw TypeError('Expected timer callback')
    const id = nextTimer++
    timers.set(id, { due: __now() + Math.max(0, Number(delay) || 0), run: () => callback(...args) })
    return id
  }
  globalThis.clearTimeout = id => { timers.delete(id) }

  const buffer = {
    from(value, encoding = 'utf8') {
      return typeof value === 'string'
        ? Uint8Array.from(__fromText(value, encoding.toLowerCase()))
        : new Uint8Array(value)
    },
    bufToString(value, encoding = 'utf8') {
      return __toText(Array.from(buffer.from(value)), encoding.toLowerCase())
    }
  }
  const bytes = value => Array.from(buffer.from(value))
  globalThis.lx = {
    utils: {
      buffer,
      crypto: {
        md5: __md5,
        randomBytes: size => Uint8Array.from(__randomBytes(size)),
        aesEncrypt: (value, mode, key, iv) =>
          Uint8Array.from(__aesEncrypt(bytes(value), mode, bytes(key), bytes(iv)))
      }
    },
    version: '2.0.0',
    env: 'desktop',
    currentScriptInfo: info,
    EVENT_NAMES: { inited: 'inited', request: 'request', updateAlert: 'updateAlert' },
    on(event, callback) {
      if (event === 'request') handler = callback
    },
    send(event, data) {
      if (event === 'inited') initialized(data.sources)
      return Promise.resolve()
    },
    request(url, options, callback) {
      let cancelled = false
      Promise.resolve().then(() => {
        if (cancelled) return
        const response = JSON.parse(__http(url, JSON.stringify(options || {})))
        if (response.error) callback(new Error(response.error))
        else callback(null, response, response.body)
      })
      return () => { cancelled = true }
    }
  }
  return {
    runNextTimer() {
      let next
      for (const entry of timers) {
        if (!next || entry[1].due < next[1].due) next = entry
      }
      if (!next) return -1
      const [id, timer] = next
      const remaining = timer.due - __now()
      if (remaining > 0) return Math.min(remaining, 2147483647)
      timers.delete(id)
      timer.run()
      return 0
    },
    initialized: ready.then(sources => {
      if (typeof handler !== 'function') throw new Error('Missing LX request handler')
      return JSON.stringify(sources)
    }),
    async resolve(event) {
      const url = await handler(JSON.parse(event))
      if (typeof url !== 'string' || !url.trim()) {
        const kind = url === null ? 'null' : typeof url
        throw new Error(`LX script returned no playback URL (result: ${kind})`)
      }
      return url
    }
  }
})()
