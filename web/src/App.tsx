import { useMemo, useRef, useState } from 'react'
import { AlertTriangle, CheckCircle2, ExternalLink, ShieldCheck, Upload } from 'lucide-react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Label } from '@/components/ui/label'
import { Separator } from '@/components/ui/separator'
import { applyPatches, detectFirmwareModel, type FirmwareModel, type PatchOutcome } from '@/lib/wasm'
import { PATCHES } from '@/lib/patches'

type Status =
  | { kind: 'idle' }
  | { kind: 'working' }
  | { kind: 'error'; message: string }
  | { kind: 'done'; outcome: PatchOutcome; fileName: string }

function downloadName(originalName: string): string {
  const base = originalName.replace(/\.bin$/i, '') || 'Zoom-L6'
  return `${base}-patched.bin`
}

function triggerDownload(bytes: Uint8Array, name: string) {
  const blob = new Blob([bytes.slice().buffer], { type: 'application/octet-stream' })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = name
  link.click()
  URL.revokeObjectURL(url)
}

export default function App() {
  const [selected, setSelected] = useState<Set<string>>(
    () => new Set(PATCHES.map((patch) => patch.id)),
  )
  const [file, setFile] = useState<File | null>(null)
  const [model, setModel] = useState<FirmwareModel | null>(null)
  const [status, setStatus] = useState<Status>({ kind: 'idle' })
  const fileInputRef = useRef<HTMLInputElement>(null)
  const fileRequest = useRef(0)

  const canApply = useMemo(
    () => file !== null && model !== null && selected.size > 0 && status.kind !== 'working',
    [file, model, selected, status],
  )

  function togglePatch(id: string, checked: boolean) {
    setSelected((previous) => {
      const next = new Set(previous)
      if (checked) next.add(id)
      else next.delete(id)
      return next
    })
  }

  async function handleFile(file: File | null) {
    const request = ++fileRequest.current
    setFile(file)
    setModel(null)
    setStatus({ kind: 'idle' })
    if (!file) return
    try {
      const bytes = new Uint8Array(await file.arrayBuffer())
      const detected = await detectFirmwareModel(bytes)
      if (request !== fileRequest.current) return
      setModel(detected)
      setSelected(new Set(PATCHES.filter((patch) => patch.models.includes(detected)).map((patch) => patch.id)))
    } catch (error) {
      if (request !== fileRequest.current) return
      setStatus({ kind: 'error', message: error instanceof Error ? error.message : String(error) })
    }
  }

  async function handleApply() {
    if (!file || !model) return
    setStatus({ kind: 'working' })
    try {
      const bytes = new Uint8Array(await file.arrayBuffer())
      const outcome = await applyPatches(bytes, [...selected])
      setStatus({ kind: 'done', outcome, fileName: file.name })
    } catch (error) {
      setStatus({
        kind: 'error',
        message: error instanceof Error ? error.message : String(error),
      })
    }
  }

  return (
    <div className="mx-auto flex min-h-svh max-w-2xl flex-col gap-8 px-4 py-10 sm:px-6">
      <header className="flex flex-col gap-3">
        <h1 className="text-3xl font-semibold tracking-tight">
          Zoom L6 / L6max Firmware Patches
        </h1>
        <p className="text-muted-foreground text-sm leading-relaxed">
          Build a patched update package for the Zoom LiveTrak L6 or L6max.
          The firmware model is detected from the uploaded package, and only
          patches supported by that model can be selected. Upload your own file and
          download the result.{' '}
          <a
            className="underline underline-offset-3 hover:text-foreground"
            href="https://github.com/derekstavis/zoom-l6max"
            target="_blank"
            rel="noreferrer"
          >
            About this project
            <ExternalLink className="ml-0.5 inline size-3.5 align-text-top" />
          </a>
        </p>
      </header>

      <Card>
        <CardHeader>
          <CardTitle>1. Choose your firmware</CardTitle>
          <CardDescription>
            <code className="text-foreground bg-muted rounded px-1 py-0.5 text-xs">
              L6.BIN or L6max.BIN
            </code>{' '}
            from the official Zoom update package — the file you'd otherwise
            copy straight to an SD card. Stock 1.10 firmware is supported. Get it from Zoom's{' '}
            <a
              className="underline underline-offset-3 hover:text-foreground"
              href="https://zoomcorp.com/en/us/digital-mixer-multi-track-recorders/digital-mixer-recorder/livetrak-l6-final/l6-support/"
              target="_blank"
              rel="noreferrer"
            >
              L6 support
            </a>{' '}
            or{' '}
            <a
              className="underline underline-offset-3 hover:text-foreground"
              href="https://zoomcorp.com/en/us/digital-mixer-multi-track-recorders/digital-mixer-recorder/livetrak-l6max/l6max-support/"
              target="_blank"
              rel="noreferrer"
            >
              L6max support
            </a>.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <input
            ref={fileInputRef}
            type="file"
            accept=".bin"
            className="hidden"
            onChange={(event) => {
              void handleFile(event.target.files?.[0] ?? null)
            }}
          />
          <button
            type="button"
            onClick={() => fileInputRef.current?.click()}
            className="border-input hover:bg-accent/50 flex w-full flex-col items-center gap-2 rounded-lg border border-dashed px-4 py-8 text-center transition-colors"
          >
            <Upload className="text-muted-foreground size-5" />
            <span className="text-sm font-medium">
              {file ? file.name : 'Click to choose a file'}
            </span>
            {model && (
              <span className="text-muted-foreground text-xs">Detected model: {model}</span>
            )}
            {file && (
              <span className="text-muted-foreground text-xs">
                {(file.size / 1024).toFixed(0)} KB
              </span>
            )}
          </button>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>2. Choose patches</CardTitle>
          <CardDescription>
            Available patches depend on the detected firmware model. Compatible
            patches can be combined. See each patch's own README for details and
            limitations.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-5">
          {PATCHES.map((patch, index) => {
            const supported = model === null || patch.models.includes(model)
            return (
              <div key={patch.id}>
                {index > 0 && <Separator className="mb-5" />}
                <div className={`flex items-start gap-3 ${supported ? '' : 'opacity-45'}`}>
                  <Checkbox
                    id={patch.id}
                    checked={selected.has(patch.id)}
                    disabled={!supported}
                    onCheckedChange={(checked) => togglePatch(patch.id, checked === true)}
                    className="mt-0.5"
                  />
                  <div className="flex flex-col gap-1">
                    <Label htmlFor={patch.id} className="text-sm font-medium">
                      {patch.title}
                    </Label>
                    <p className="text-muted-foreground text-sm leading-relaxed">
                      {patch.description} Supported: {patch.models.join(', ')}.
                      {!supported && ` Not available for ${model}.`}
                    </p>
                    {patch.demoGif && (
                      <img
                        src={`${import.meta.env.BASE_URL}demos/${patch.demoGif}`}
                        alt={`${patch.title} demo`}
                        className="mt-1 w-fit max-w-full rounded-md border"
                        loading="lazy"
                      />
                    )}
                    {patch.requestCommand && (
                      <p className="bg-muted text-muted-foreground mt-1 w-fit rounded-md px-2 py-1 text-xs leading-relaxed">
                        {patch.requestCommand}
                      </p>
                    )}
                    <a
                      className="text-muted-foreground w-fit text-xs underline underline-offset-3 hover:text-foreground"
                      href={patch.readmeUrl}
                      target="_blank"
                      rel="noreferrer"
                    >
                      Read the full write-up
                    </a>
                  </div>
                </div>
              </div>
            )
          })}
        </CardContent>
      </Card>

      <Button size="lg" disabled={!canApply} onClick={handleApply} className="w-full">
        {status.kind === 'working' ? 'Patching…' : 'Apply patches'}
      </Button>

      {status.kind === 'error' && (
        <Alert variant="destructive">
          <AlertTriangle />
          <AlertTitle>Couldn't patch this file</AlertTitle>
          <AlertDescription>{status.message}</AlertDescription>
        </Alert>
      )}

      {status.kind === 'done' && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <CheckCircle2 className="size-5 text-emerald-600 dark:text-emerald-400" />
              Patched successfully
            </CardTitle>
            <CardDescription>
              Version marker {status.outcome.report.version} · image{' '}
              {status.outcome.report.image_length.toLocaleString()} bytes
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            <Button
              onClick={() =>
                triggerDownload(status.outcome.package, downloadName(status.fileName))
              }
            >
              Download {downloadName(status.fileName)}
            </Button>
            <div className="text-muted-foreground flex flex-col gap-1 text-xs">
              <span>Applied: {status.outcome.report.patches.map((p) => p.name).join(', ')}</span>
              <span className="break-all">
                Package SHA-256: {status.outcome.report.update_sha256}
              </span>
            </div>
            <Alert>
              <ShieldCheck />
              <AlertTitle>Before you install it</AlertTitle>
              <AlertDescription>
                {status.outcome.report.usage} Install the same way as any
                official update (SD card), and keep your original firmware
                file.
              </AlertDescription>
            </Alert>
          </CardContent>
        </Card>
      )}

      <footer className="text-muted-foreground mt-4 text-center text-xs">
        Not affiliated with or endorsed by Zoom Corporation. Experimental,
        independently developed firmware patches — use at your own risk.
      </footer>
    </div>
  )
}
