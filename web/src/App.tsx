import { useMemo, useRef, useState } from 'react'
import { zipSync } from 'fflate'
import { AlertTriangle, CheckCircle2, Download, ExternalLink, Upload } from 'lucide-react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader } from '@/components/ui/card'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { applyPatches, detectFirmwareModel, type FirmwareModel, type PatchOutcome } from '@/lib/wasm'
import { LOADABLE_PATCHES, PATCHES } from '@/lib/patches'

type Status =
  | { kind: 'idle' }
  | { kind: 'working' }
  | { kind: 'error'; message: string }
  | { kind: 'done'; outcome: PatchOutcome; fileName: string }

type ArchiveStatus =
  | { kind: 'idle' }
  | { kind: 'working' }
  | { kind: 'error'; message: string }

function triggerDownload(bytes: Uint8Array, name: string) {
  const blob = new Blob([bytes.slice().buffer], { type: 'application/octet-stream' })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = name
  link.click()
  URL.revokeObjectURL(url)
}

function ModelSupportTags({ models }: { models: readonly FirmwareModel[] }) {
  return (
    <div className="mt-1 flex flex-wrap items-center gap-1.5 text-xs" aria-label="Supported models">
      <span className="text-muted-foreground">Works on</span>
      {(['L6', 'L6max'] as const).map((model) => {
        const supported = models.includes(model)
        return (
          <span
            key={model}
            aria-label={`${model}: ${supported ? 'supported' : 'not supported'}`}
            className={`inline-flex rounded-full border px-2 py-0.5 font-medium ${supported
              ? 'border-emerald-200 bg-emerald-50 text-emerald-800 dark:border-emerald-900 dark:bg-emerald-950/50 dark:text-emerald-300'
              : 'border-border bg-muted text-muted-foreground opacity-60'
            }`}
          >
            {model}
          </span>
        )
      })}
    </div>
  )
}

export default function App() {
  const [selected, setSelected] = useState<Set<string>>(
    () => new Set(PATCHES.map((patch) => patch.id)),
  )
  const [loadFromSd, setLoadFromSd] = useState<Set<string>>(
    () => new Set(LOADABLE_PATCHES.map((patch) => patch.id)),
  )
  const [file, setFile] = useState<File | null>(null)
  const [model, setModel] = useState<FirmwareModel | null>(null)
  const [status, setStatus] = useState<Status>({ kind: 'idle' })
  const [archiveStatus, setArchiveStatus] = useState<ArchiveStatus>({ kind: 'idle' })
  const fileInputRef = useRef<HTMLInputElement>(null)
  const fileRequest = useRef(0)
  const installPatchLoader = [...selected].some((id) => loadFromSd.has(id))
  const sdPatchesToDownload = model
    ? LOADABLE_PATCHES.filter(
        (patch) =>
          patch.models.includes(model) && selected.has(patch.id) && loadFromSd.has(patch.id),
      )
    : []

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
    setStatus({ kind: 'idle' })
    setArchiveStatus({ kind: 'idle' })
  }

  function choosePatchDelivery(id: string, fromSd: boolean) {
    setLoadFromSd((previous) => {
      const next = new Set(previous)
      if (fromSd) next.add(id)
      else next.delete(id)
      return next
    })
    setStatus({ kind: 'idle' })
    setArchiveStatus({ kind: 'idle' })
  }

  async function handleFile(file: File | null) {
    const request = ++fileRequest.current
    setFile(file)
    setModel(null)
    setStatus({ kind: 'idle' })
    setArchiveStatus({ kind: 'idle' })
    if (!file) return
    try {
      const bytes = new Uint8Array(await file.arrayBuffer())
      const detected = await detectFirmwareModel(bytes)
      if (request !== fileRequest.current) return
      setModel(detected)
      setSelected(new Set(PATCHES.filter((patch) => patch.models.includes(detected)).map((patch) => patch.id)))
      setLoadFromSd(new Set(LOADABLE_PATCHES.filter((patch) => patch.models.includes(detected)).map((patch) => patch.id)))
    } catch (error) {
      if (request !== fileRequest.current) return
      setStatus({ kind: 'error', message: error instanceof Error ? error.message : String(error) })
    }
  }

  async function handleApply() {
    if (!file || !model) return
    setStatus({ kind: 'working' })
    setArchiveStatus({ kind: 'idle' })
    try {
      const bytes = new Uint8Array(await file.arrayBuffer())
      const firmwarePatches = [...selected].filter((id) => !loadFromSd.has(id))
      const outcome = await applyPatches(bytes, firmwarePatches, installPatchLoader)
      setStatus({ kind: 'done', outcome, fileName: file.name })
    } catch (error) {
      setStatus({
        kind: 'error',
        message: error instanceof Error ? error.message : String(error),
      })
    }
  }

  async function handleDownloadPackageZip() {
    if (!model || status.kind !== 'done') return
    setArchiveStatus({ kind: 'working' })
    try {
      const entries = await Promise.all(
        sdPatchesToDownload.map(async (patch) => {
          const response = await fetch(
            `${import.meta.env.BASE_URL}loadable-patches/${model}/${patch.fileName}`,
          )
          if (!response.ok) throw new Error(`Couldn't load ${patch.title}.`)
          const prefix = String(patch.order).padStart(2, '0')
          return [`PATCHES/${prefix}-${patch.fileName}`, new Uint8Array(await response.arrayBuffer())] as const
        }),
      )
      const files: Record<string, Uint8Array> = Object.fromEntries(entries)
      files[`${model}.BIN`] = status.outcome.package
      const archive = zipSync(files, { level: 0 })
      triggerDownload(archive, `${model}-firmware-update.zip`)
      setArchiveStatus({ kind: 'idle' })
    } catch (error) {
      setArchiveStatus({
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
          Add features to your Zoom L6 or L6max through a firmware update.{' '}
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

      <section className="flex flex-col gap-4" aria-labelledby="firmware-selection-title">
        <div className="flex flex-col gap-1">
          <h2 id="firmware-selection-title" className="text-xl font-semibold tracking-tight">
            Upload the original firmware
          </h2>
          <p className="text-muted-foreground text-sm leading-relaxed">
            Download the firmware update for your mixer from Zoom (
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
            </a>
            ) and drop it here.
          </p>
        </div>
        <Card>
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
      </section>

      <section className="flex flex-col gap-4" aria-labelledby="patch-selection-title">
        <div className="flex flex-col gap-1">
          <h2 id="patch-selection-title" className="text-xl font-semibold tracking-tight">
            2. Choose the patches
          </h2>
          <p className="text-muted-foreground text-sm leading-relaxed">
            Choose one or more patches, then choose whether each one is installed in the firmware or from the SD card.
          </p>
        </div>
        {installPatchLoader && (
          <Alert className="border-blue-200 bg-blue-50 text-blue-950 dark:border-blue-900 dark:bg-blue-950/40 dark:text-blue-100">
            <AlertTitle>Built-in vs SD card</AlertTitle>
            <AlertDescription className="text-blue-800 dark:text-blue-200">
              Reverting a built-in patch requires reverting to the original firmware, while SD card patches can be reverted by removing the SD card or the patch file.
            </AlertDescription>
          </Alert>
        )}
        {PATCHES.map((patch) => {
            const supported = model === null || patch.models.includes(model)
            const patchEnabled = selected.has(patch.id)
            const loadablePatch = LOADABLE_PATCHES.find((candidate) => candidate.id === patch.id)
            const canLoadFromSd = supported && patchEnabled && loadablePatch !== undefined
            const selectedForSd = canLoadFromSd && loadFromSd.has(patch.id)
            return (
              <Card key={patch.id} className="gap-0">
                <CardHeader className={supported ? '' : 'opacity-45'}>
                  <div className="flex items-center justify-between gap-3">
                    <Label htmlFor={patch.id} className="text-sm font-medium">
                      {patch.title}
                    </Label>
                    <Switch
                      id={patch.id}
                      aria-label={`Enable ${patch.title}`}
                      checked={patchEnabled}
                      disabled={!supported}
                      onCheckedChange={(checked) => togglePatch(patch.id, checked)}
                    />
                  </div>
                </CardHeader>
                <div
                  className={`grid transition-[grid-template-rows,opacity] duration-200 ease-in-out motion-reduce:transition-none ${patchEnabled ? 'grid-rows-[1fr] opacity-100' : 'grid-rows-[0fr] opacity-0'}`}
                  aria-hidden={!patchEnabled}
                  inert={!patchEnabled}
                >
                  <div className="min-h-0 overflow-hidden">
                    <CardContent className={`flex flex-col gap-1 pt-3 ${supported ? '' : 'opacity-45'}`}>
                      <p className="text-muted-foreground text-sm leading-relaxed">{patch.description}</p>
                      <ModelSupportTags models={patch.models} />
                      {'demoImage' in patch && patch.demoImage && (
                        <img
                          src={`${import.meta.env.BASE_URL}demos/${patch.demoImage}`}
                          alt={`${patch.title} demo`}
                          className="mt-1 w-fit max-w-full rounded-md border"
                          loading="lazy"
                        />
                      )}
                      {'requestCommand' in patch && patch.requestCommand && (
                        <p className="bg-muted text-muted-foreground mt-1 w-fit rounded-md px-2 py-1 text-xs leading-relaxed">
                          {patch.requestCommand}
                        </p>
                      )}
                      {!supported && (
                        <p className="text-muted-foreground text-xs">Not available for {model}.</p>
                      )}
                      <div className="mt-1 flex items-center justify-between gap-4">
                        <span className="text-xs font-medium">Installation method</span>
                        <a
                          className="text-muted-foreground text-right text-xs underline underline-offset-3 hover:text-foreground"
                          href={patch.readmeUrl}
                          target="_blank"
                          rel="noreferrer"
                        >
                          Technical details
                        </a>
                      </div>
                      <div className="inline-flex w-fit rounded-md border p-0.5" role="group" aria-label={`Installation method for ${patch.title}`}>
                        <button type="button" disabled={!canLoadFromSd} aria-pressed={!selectedForSd} onClick={() => choosePatchDelivery(patch.id, false)} className={`rounded px-3 py-1.5 text-xs font-medium ${!selectedForSd ? 'bg-primary text-primary-foreground' : 'text-muted-foreground hover:text-foreground'} disabled:cursor-not-allowed disabled:opacity-50`}>
                        Firmware
                        </button>
                        <button type="button" disabled={!canLoadFromSd} aria-pressed={selectedForSd} onClick={() => choosePatchDelivery(patch.id, true)} className={`rounded px-3 py-1.5 text-xs font-medium ${selectedForSd ? 'bg-primary text-primary-foreground' : 'text-muted-foreground hover:text-foreground'} disabled:cursor-not-allowed disabled:opacity-50`}>
                        SD card
                        </button>
                      </div>
                      {!loadablePatch && (
                        <p className="text-muted-foreground text-xs">Only available as a firmware patch.</p>
                      )}
                    </CardContent>
                  </div>
                </div>
              </Card>
            )
          })}
      </section>

      <section className="flex flex-col gap-4" aria-labelledby="download-update-title">
        <div className="flex flex-col gap-1">
          <h2 id="download-update-title" className="text-xl font-semibold tracking-tight">
            Download the modded package
          </h2>
          <p className="text-muted-foreground text-sm leading-relaxed">
            Create your custom firmware, then download the installable package.
          </p>
        </div>
        <Card>
          <CardContent className="flex flex-col gap-4">
            <Button size="lg" disabled={!canApply} onClick={handleApply} className="w-full">
              {status.kind === 'working' ? 'Preparing…' : 'Create firmware update'}
            </Button>
            {status.kind === 'error' && (
              <Alert variant="destructive">
                <AlertTriangle />
                <AlertTitle>Couldn't create the update</AlertTitle>
                <AlertDescription>{status.message}</AlertDescription>
              </Alert>
            )}
            {status.kind === 'done' && (
              <>
                <div className="flex items-center gap-2 text-sm font-medium">
                  <CheckCircle2 className="size-5 text-emerald-600 dark:text-emerald-400" />
                  Your firmware update is ready. Download it as a ZIP for your SD card.
                </div>
                <Button
                  disabled={archiveStatus.kind === 'working'}
                  onClick={() => void handleDownloadPackageZip()}
                >
                  <Download className="size-4" />
                  {archiveStatus.kind === 'working' ? 'Preparing ZIP…' : 'Download ZIP for SD card'}
                </Button>
              </>
            )}
            {archiveStatus.kind === 'error' && (
              <Alert variant="destructive">
                <AlertTriangle />
                <AlertTitle>Couldn't prepare the ZIP</AlertTitle>
                <AlertDescription>{archiveStatus.message}</AlertDescription>
              </Alert>
            )}
          </CardContent>
        </Card>
      </section>

      <section className="flex flex-col gap-4" aria-labelledby="apply-update-title">
        <div className="flex flex-col gap-1">
          <h2 id="apply-update-title" className="text-xl font-semibold tracking-tight">
            4. Install the update on your mixer
          </h2>
          <p className="text-muted-foreground text-sm leading-relaxed">
            Follow the steps for your model. Keep the mixer powered and leave the card in place until the update finishes.
          </p>
        </div>
        <Card>
          <CardContent className="flex flex-col gap-3">
            {model && (
              <p className="text-sm leading-relaxed">
                Unzip the file from step 3 into the main folder of your SD card, then follow the steps below. It contains the firmware update{installPatchLoader && <> and a <code>PATCHES</code> folder with your SD card patches</>}.
              </p>
            )}
            {model === null ? (
              <p className="text-muted-foreground text-sm">Upload your firmware above to see the instructions for your mixer.</p>
            ) : model === 'L6' ? (
              <>
                <ol className="text-sm leading-relaxed list-decimal space-y-2 pl-5">
                  <li>Connect power and insert the card.</li>
                  <li>Hold Play/Stop, then press and hold Power until the power light turns on.</li>
                  <li>Wait for the power light to blink rapidly green. Do not turn off the mixer or remove the card while it updates.</li>
                  <li>When the update is complete, turn the mixer off.</li>
                </ol>
                <a className="text-muted-foreground w-fit text-xs underline underline-offset-3 hover:text-foreground" href="https://zoomcorp.com/documents/3579/L6_Firmware_Update_Guide_EN.pdf" target="_blank" rel="noreferrer">
                  View Zoom&apos;s L6 update guide
                </a>
              </>
            ) : (
              <>
                <ol className="text-sm leading-relaxed list-decimal space-y-2 pl-5">
                  <li>Connect power, insert the card, and turn on the mixer. When asked, choose Execute to prepare the update. If not asked, open System → Firmware → Firmware Update from the menu.</li>
                  <li>Turn the mixer off, then turn it on again to start the update. Wait for the completion screen. Do not turn it off or remove the card while it updates.</li>
                  <li>Turn the mixer off after the update completes.</li>
                </ol>
                <a className="text-muted-foreground w-fit text-xs underline underline-offset-3 hover:text-foreground" href="https://zoomcorp.com/media/documents/L6max_Firmware_Update_Guide_EN.pdf" target="_blank" rel="noreferrer">
                  View Zoom&apos;s L6max update guide
                </a>
              </>
            )}
          </CardContent>
        </Card>
      </section>

      <footer className="text-muted-foreground mt-4 text-center text-xs">
        Not affiliated with or endorsed by Zoom Corporation. Experimental,
        independently developed firmware patches — use at your own risk.
      </footer>
    </div>
  )
}
