// If you want to use Phoenix channels, run `mix help phx.gen.channel`
// to get started and then uncomment the line below.
// import "./user_socket.js"

// You can include dependencies in two ways.
//
// The simplest option is to put them in assets/vendor and
// import them using relative paths:
//
//     import "../vendor/some-package.js"
//
// Alternatively, you can `npm install some-package --prefix assets` and import
// them using a path starting with the package name:
//
//     import "some-package"
//
// If you have dependencies that try to import CSS, esbuild will generate a separate `app.css` file.
// To load it, simply add a second `<link>` to your `root.html.heex` file.

// Include phoenix_html to handle method=PUT/DELETE in forms and buttons.
import "phoenix_html"
// Establish Phoenix Socket and LiveView configuration.
import {Socket} from "phoenix"
import {LiveSocket} from "phoenix_live_view"
import {hooks as colocatedHooks} from "phoenix-colocated/styra_web"
import topbar from "../vendor/topbar"

const MAX_RECORDING_MS = 120_000
const WAV_SAMPLE_RATE = 16_000

const VoiceRecorder = {
  mounted() {
    this.capture = null
    this.recordingTimer = null
    this.onClick = () => this.toggleRecording()
    this.el.addEventListener("click", this.onClick)
    this.handleEvent("voice-finished", () => this.resetRecorder())
    this.handleEvent("voice-failed", () => this.resetRecorder())
  },

  destroyed() {
    this.el.removeEventListener("click", this.onClick)
    this.discardRecording()
  },

  async toggleRecording() {
    if (this.capture?.recorder.state === "recording") {
      await this.finishRecording()
    } else if (this.el.dataset.state === "idle") {
      await this.startRecording()
    }
  },

  async startRecording() {
    try {
      if (!navigator.mediaDevices?.getUserMedia || !window.MediaRecorder) {
        throw new Error("This browser does not support microphone recording.")
      }

      this.setState("busy", "Waiting for microphone permission")
      const stream = await navigator.mediaDevices.getUserMedia({audio: true})
      const recorder = new MediaRecorder(stream, preferredRecorderOptions())
      const capture = {stream, recorder, chunks: []}
      recorder.addEventListener("dataavailable", event => {
        if (event.data.size > 0) capture.chunks.push(event.data)
      })
      this.capture = capture
      recorder.start()
      this.setState("recording", "Stop recording")
      this.pushEvent("audio_recording_started", {})
      this.recordingTimer = window.setTimeout(() => this.finishRecording(), MAX_RECORDING_MS)
    } catch (error) {
      this.failRecording(readableRecordingError(error))
    }
  },

  async finishRecording() {
    const capture = this.capture
    if (!capture || capture.recorder.state !== "recording") return

    try {
      this.setState("busy", "Preparing voice message")
      window.clearTimeout(this.recordingTimer)
      const stopped = new Promise(resolve => capture.recorder.addEventListener("stop", resolve, {once: true}))
      capture.recorder.stop()
      await stopped
      this.releaseCapture(capture)
      const message = document.querySelector('[name="message[text]"]')
      const selectionStart = message?.selectionStart ?? message?.value.length ?? 0
      const selectionEnd = message?.selectionEnd ?? selectionStart
      const contract = document.querySelector('[name="message[contract]"]')?.value || "none"
      this.pushEvent("audio_recording_stopped", {
        contract,
        before: message?.value.slice(0, selectionStart) || "",
        after: message?.value.slice(selectionEnd) || "",
      })

      if (capture.chunks.length === 0) throw new Error("The microphone did not produce any audio.")

      const captured = new Blob(capture.chunks, {type: capture.recorder.mimeType})
      const wav = await recordingToWav(captured)
      const file = new File([wav], "voice-message.wav", {type: "audio/wav"})
      this.upload("audio", [file])
    } catch (error) {
      this.failRecording(readableRecordingError(error))
    }
  },

  failRecording(message) {
    this.discardRecording()
    this.setState("idle", "Record a voice message")
    this.pushEvent("audio_recording_error", {error: message})
  },

  discardRecording() {
    window.clearTimeout(this.recordingTimer)
    const capture = this.capture
    if (!capture) return
    if (capture.recorder.state === "recording") capture.recorder.stop()
    this.releaseCapture(capture)
  },

  releaseCapture(capture) {
    capture.stream.getTracks().forEach(track => track.stop())
    if (this.capture === capture) this.capture = null
  },

  resetRecorder() {
    if (this.capture?.recorder.state === "recording") return
    this.setState("idle", "Record a voice message")
  },

  setState(state, label) {
    this.el.dataset.state = state
    this.el.disabled = state === "busy"
    this.el.setAttribute("aria-label", label)
    this.el.title = label
  },
}

function preferredRecorderOptions() {
  const mimeTypes = ["audio/webm;codecs=opus", "audio/ogg;codecs=opus", "audio/mp4"]
  const mimeType = mimeTypes.find(type => MediaRecorder.isTypeSupported(type))
  return mimeType ? {mimeType} : {}
}

async function recordingToWav(blob) {
  const AudioContext = window.AudioContext || window.webkitAudioContext
  if (!AudioContext) throw new Error("This browser cannot prepare recorded audio.")

  const context = new AudioContext()
  try {
    const decoded = await context.decodeAudioData(await blob.arrayBuffer())
    return encodeMonoWav(decoded, WAV_SAMPLE_RATE)
  } finally {
    await context.close()
  }
}

function encodeMonoWav(audio, outputRate) {
  const sampleCount = Math.max(1, Math.floor(audio.duration * outputRate))
  const bytes = new ArrayBuffer(44 + sampleCount * 2)
  const view = new DataView(bytes)
  writeAscii(view, 0, "RIFF")
  view.setUint32(4, 36 + sampleCount * 2, true)
  writeAscii(view, 8, "WAVEfmt ")
  view.setUint32(16, 16, true)
  view.setUint16(20, 1, true)
  view.setUint16(22, 1, true)
  view.setUint32(24, outputRate, true)
  view.setUint32(28, outputRate * 2, true)
  view.setUint16(32, 2, true)
  view.setUint16(34, 16, true)
  writeAscii(view, 36, "data")
  view.setUint32(40, sampleCount * 2, true)

  const channels = Array.from({length: audio.numberOfChannels}, (_, index) => audio.getChannelData(index))
  const sourceStep = audio.sampleRate / outputRate
  for (let index = 0; index < sampleCount; index++) {
    const sourceIndex = Math.min(Math.floor(index * sourceStep), audio.length - 1)
    const sample = channels.reduce((sum, channel) => sum + channel[sourceIndex], 0) / channels.length
    const clipped = Math.max(-1, Math.min(1, sample))
    view.setInt16(44 + index * 2, clipped < 0 ? clipped * 0x8000 : clipped * 0x7fff, true)
  }

  return new Blob([bytes], {type: "audio/wav"})
}

function writeAscii(view, offset, value) {
  for (let index = 0; index < value.length; index++) view.setUint8(offset + index, value.charCodeAt(index))
}

function readableRecordingError(error) {
  if (error?.name === "NotAllowedError") return "Microphone access was denied. Allow it in your browser and try again."
  if (error?.name === "NotFoundError") return "No microphone was found."
  return error?.message || "Could not record a voice message."
}

const csrfToken = document.querySelector("meta[name='csrf-token']").getAttribute("content")
const liveSocket = new LiveSocket("/live", Socket, {
  longPollFallbackMs: 2500,
  params: {_csrf_token: csrfToken},
  hooks: {...colocatedHooks, VoiceRecorder},
})

// Show progress bar on live navigation and form submits
topbar.config({barColors: {0: "#29d"}, shadowColor: "rgba(0, 0, 0, .3)"})
window.addEventListener("phx:page-loading-start", _info => topbar.show(300))
window.addEventListener("phx:page-loading-stop", _info => topbar.hide())

// connect if there are any LiveViews on the page
liveSocket.connect()

// expose liveSocket on window for web console debug logs and latency simulation:
// >> liveSocket.enableDebug()
// >> liveSocket.enableLatencySim(1000)  // enabled for duration of browser session
// >> liveSocket.disableLatencySim()
window.liveSocket = liveSocket

// The lines below enable quality of life phoenix_live_reload
// development features:
//
//     1. stream server logs to the browser console
//     2. click on elements to jump to their definitions in your code editor
//
if (process.env.NODE_ENV === "development") {
  window.addEventListener("phx:live_reload:attached", ({detail: reloader}) => {
    // Enable server log streaming to client.
    // Disable with reloader.disableServerLogs()
    reloader.enableServerLogs()

    // Open configured PLUG_EDITOR at file:line of the clicked element's HEEx component
    //
    //   * click with "c" key pressed to open at caller location
    //   * click with "d" key pressed to open at function component definition location
    let keyDown
    window.addEventListener("keydown", e => keyDown = e.key)
    window.addEventListener("keyup", _e => keyDown = null)
    window.addEventListener("click", e => {
      if(keyDown === "c"){
        e.preventDefault()
        e.stopImmediatePropagation()
        reloader.openEditorAtCaller(e.target)
      } else if(keyDown === "d"){
        e.preventDefault()
        e.stopImmediatePropagation()
        reloader.openEditorAtDef(e.target)
      }
    }, true)

    window.liveReloader = reloader
  })
}
