import assert from 'node:assert/strict'
import { access, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const require = createRequire(import.meta.url)
const extract = require('extract-zip')
const malicious = 'UEsDBBQAAAAAAAAAIQBASv+xDQAAAA0AAAAGAAAAZXNjYXBlLi4vLi4vb3V0c2lkZVBLAQIUAxQAAAAAAAAAIQBASv+xDQAAAA0AAAAGAAAAAAAAAAAAAAD/oQAAAABlc2NhcGVQSwUGAAAAAAEAAQA0AAAAMQAAAAAA'
const traversal = 'UEsDBBQAAAAAAB0OFV0tU7Q7BQAAAAUAAAAJAAAALi4vZXNjYXBlb3duZWRQSwECFAMUAAAAAAAdDhVdLVO0OwUAAAAFAAAACQAAAAAAAAAAAAAAgAEAAAAALi4vZXNjYXBlUEsFBgAAAAABAAEANwAAACwAAAAAAA=='
const safe = 'UEsDBBQAAAAAAC0NFV2PKKQfBAAAAAQAAAAIAAAAc2FmZS50eHRzYWZlUEsBAhQDFAAAAAAALQ0VXY8opB8EAAAABAAAAAgAAAAAAAAAAAAAAIABAAAAAHNhZmUudHh0UEsFBgAAAAABAAEANgAAACoAAAAAAA=='
const nested = 'UEsDBBQAAAgIAFwPFV2PKKQfBgAAAAQAAAAXAAAAbGlua2VkL2NyZWF0ZWQvZmlsZS50eHQrTkxLBQBQSwECFAMUAAAICABcDxVdjyikHwYAAAAEAAAAFwAAAAAAAAAAAAAApIEAAAAAbGlua2VkL2NyZWF0ZWQvZmlsZS50eHRQSwUGAAAAAAEAAQBFAAAAOwAAAAAA'
const root = await mkdtemp(join(tmpdir(), 'hzr-safe-extract-'))

try {
  const safeArchive = join(root, 'safe.zip')
  const safeOutput = join(root, 'safe')
  await writeFile(safeArchive, Buffer.from(safe, 'base64'))
  await extract(safeArchive, { dir: safeOutput })
  assert.equal(await readFile(join(safeOutput, 'safe.txt'), 'utf8'), 'safe')

  const maliciousArchive = join(root, 'malicious.zip')
  const maliciousOutput = join(root, 'malicious')
  await writeFile(maliciousArchive, Buffer.from(malicious, 'base64'))
  await assert.rejects(
    extract(maliciousArchive, { dir: maliciousOutput }),
    /symlink target .* escapes extraction root/
  )

  const traversalArchive = join(root, 'traversal.zip')
  const traversalOutput = join(root, 'traversal')
  await writeFile(traversalArchive, Buffer.from(traversal, 'base64'))
  await assert.rejects(
    extract(traversalArchive, { dir: traversalOutput }),
    /invalid relative path|zip entry .* escapes extraction root/
  )

  const linkedArchive = join(root, 'linked.zip')
  const linkedOutput = join(root, 'linked-output')
  const outside = join(root, 'outside')
  await writeFile(linkedArchive, Buffer.from(nested, 'base64'))
  await mkdir(linkedOutput)
  await mkdir(outside)
  await symlink(outside, join(linkedOutput, 'linked'), 'dir')
  await assert.rejects(
    extract(linkedArchive, { dir: linkedOutput }),
    /Target directory must be empty|non-directory component/
  )
  await assert.rejects(
    access(join(outside, 'created')),
    undefined,
    'rejected extraction must not create directories outside root'
  )
} finally {
  await rm(root, { recursive: true, force: true })
}
