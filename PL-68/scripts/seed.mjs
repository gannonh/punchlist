// Creates 500 issues through POST /api/issues as the run's person. Usage: node seed.mjs <SERVER_URL> <CONFIG>
import { readFileSync } from 'node:fs'
const [server, config] = process.argv.slice(2)
const token = /^token = "(.*)"$/m.exec(readFileSync(config, 'utf8'))[1]
for (let i = 1; i <= 500; i++) {
  const r = await fetch(`${server}/api/issues`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'content-type': 'application/json' },
    body: JSON.stringify({ title: `Seeded issue ${i}`, body: '' }),
  })
  if (r.status !== 201) throw new Error(`issue ${i}: ${r.status}`)
}
console.log('created 500')
