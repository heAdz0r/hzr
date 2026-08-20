'use strict'

const { createWriteStream, promises: fs } = require('fs')
const path = require('path')
const { pipeline } = require('stream/promises')
const { promisify } = require('util')
const yauzl = require('yauzl')

const openZip = promisify(yauzl.open)
const MAX_SYMLINK_TARGET_BYTES = 4096

function assertContained (root, candidate, label) {
  const relative = path.relative(root, candidate)
  if (relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new Error(`${label} escapes extraction root`)
  }
}

async function ensureSafeDirectory (root, directory, mode) {
  assertContained(root, directory, 'zip entry directory')
  const relative = path.relative(root, directory)
  if (relative === '') return

  let current = root
  for (const component of relative.split(path.sep)) {
    current = path.join(current, component)
    try {
      const metadata = await fs.lstat(current)
      if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
        throw new Error(`zip entry directory contains a non-directory component: ${current}`)
      }
    } catch (error) {
      if (error.code !== 'ENOENT') throw error
      await fs.mkdir(current, { mode })
      const metadata = await fs.lstat(current)
      if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
        throw new Error(`zip entry directory was replaced during extraction: ${current}`)
      }
    }
    const canonical = await fs.realpath(current)
    assertContained(root, canonical, 'zip entry directory')
  }
}

function entryMode (entry) {
  return (entry.externalFileAttributes >> 16) & 0xFFFF
}

function entryKind (entry) {
  const mode = entryMode(entry)
  const type = mode & 0o170000
  const madeBy = entry.versionMadeBy >> 8
  return {
    mode,
    isDirectory: type === 0o040000 || entry.fileName.endsWith('/') ||
      (madeBy === 0 && entry.externalFileAttributes === 16),
    isSymlink: type === 0o120000
  }
}

function extractedMode (mode, isDirectory, options) {
  if (mode !== 0) return mode & 0o777
  const fallback = isDirectory ? options.defaultDirMode : options.defaultFileMode
  return Number.parseInt(fallback, 10) || (isDirectory ? 0o755 : 0o644)
}

async function readSymlinkTarget (stream, entry) {
  if (entry.uncompressedSize > MAX_SYMLINK_TARGET_BYTES) {
    throw new Error('symlink target exceeds 4096 bytes')
  }
  const chunks = await new Promise((resolve, reject) => {
    const collected = []
    let total = 0
    stream.on('data', chunk => {
      total += chunk.length
      if (total > MAX_SYMLINK_TARGET_BYTES) {
        reject(new Error('symlink target exceeds 4096 bytes'))
        stream.destroy()
        return
      }
      collected.push(chunk)
    })
    stream.once('end', () => resolve(collected))
    stream.once('error', reject)
  })
  const target = Buffer.concat(chunks).toString('utf8')
  if (target.length === 0 || target.includes('\0') || path.isAbsolute(target)) {
    throw new Error('symlink target is invalid or absolute')
  }
  return target
}

class Extractor {
  constructor (zipPath, options) {
    this.zipPath = zipPath
    this.options = options
    this.cancelled = false
  }

  async extract () {
    this.zipfile = await openZip(this.zipPath, { lazyEntries: true })
    return new Promise((resolve, reject) => {
      const fail = error => {
        if (this.cancelled) return
        this.cancelled = true
        reject(error)
        try {
          this.zipfile.close()
        } catch {
          // Rejection above is authoritative; close is best-effort after a parser failure.
        }
      }
      this.zipfile.on('error', fail)
      this.zipfile.on('close', () => {
        if (!this.cancelled) resolve()
      })
      this.zipfile.on('entry', entry => {
        this.extractEntry(entry)
          .then(() => {
            if (!this.cancelled) this.zipfile.readEntry()
          })
          .catch(fail)
      })
      this.zipfile.readEntry()
    })
  }

  async extractEntry (entry) {
    if (this.cancelled || entry.fileName.startsWith('__MACOSX/')) return
    if (entry.fileName.includes('\0')) throw new Error('zip entry contains NUL')
    if (this.options.onEntry) this.options.onEntry(entry, this.zipfile)

    const destination = path.resolve(this.options.dir, entry.fileName)
    assertContained(this.options.dir, destination, `zip entry ${entry.fileName}`)
    const kind = entryKind(entry)
    const parent = kind.isDirectory ? destination : path.dirname(destination)
    await ensureSafeDirectory(
      this.options.dir,
      parent,
      kind.isDirectory ? extractedMode(kind.mode, true, this.options) : undefined
    )
    if (kind.isDirectory) return

    const readStream = await promisify(this.zipfile.openReadStream.bind(this.zipfile))(entry)
    if (kind.isSymlink) {
      const target = await readSymlinkTarget(readStream, entry)
      const resolvedTarget = path.resolve(path.dirname(destination), target)
      assertContained(this.options.dir, resolvedTarget, `symlink target for ${entry.fileName}`)
      await fs.symlink(target, destination)
      return
    }

    await pipeline(
      readStream,
      createWriteStream(destination, {
        flags: 'wx',
        mode: extractedMode(kind.mode, false, this.options)
      })
    )
  }
}

module.exports = async function extract (zipPath, options) {
  if (!options || !path.isAbsolute(options.dir)) {
    throw new Error('Target directory is expected to be absolute')
  }
  await fs.mkdir(options.dir, { recursive: true })
  const root = await fs.realpath(options.dir)
  if ((await fs.readdir(root)).length !== 0) {
    throw new Error('Target directory must be empty before extraction')
  }
  return new Extractor(zipPath, { ...options, dir: root }).extract()
}
