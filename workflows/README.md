# 🔄 n8n Workflows for Academic Agent

Direktori ini berisi file workflow visual **n8n** siap impor untuk mengorkestrasi seluruh siklus hidup penugasan akademik, silabus, dan notifikasi terjadwal.

---

## 📦 File Workflow

| File | Deskripsi |
| :--- | :--- |
| **`academic-agent.n8n.json`** | Workflow produksi: **Webhook Trigger ➡️ Text Normalizer ➡️ 9Router AI Extraction ➡️ iCalendar & GCal Builder ➡️ Discord Rich Embed + Action Button ➡️ Webhook Response**. |

---

## 🚀 Cara Import ke n8n (1-Click Import)

1. Buka dashboard n8n Anda (misal `http://localhost:5678` atau instance n8n Cloud Anda).
2. Di menu kiri, pilih **Workflows** ➡️ Klik **Add Workflow** (atau icon **`+`**).
3. Klik menu titik tiga (**`⋮`**) di pojok kanan atas kanvas ➡️ pilih **Import from File...**
4. Pilih file [`workflows/academic-agent.n8n.json`](./academic-agent.n8n.json).
5. Workflow akan otomatis ter-render lengkap dengan koneksi antar node!

---

## ⚙️ Variabel Lingkungan di n8n

Sebelum mengaktifkan workflow, pastikan environment variables berikut telah diatur di instance n8n Anda (atau via file `.env`):

| Variabel | Contoh Nilai | Keterangan |
| :--- | :--- | :--- |
| `ROUTER_API_BASE` | `http://host.docker.internal:20128/v1` | Endpoint 9Router / OpenAI Compatible |
| `ROUTER_API_KEY` | `your_router_api_key_here` | API Key 9Router / LLM |
| `ROUTER_MODEL` | `mr-loverman` atau `coding-general` | Nama model AI yang digunakan |
| `DISCORD_BOT_TOKEN` | `your_discord_bot_token_here` | Token Bot Discord |
| `DISCORD_CHANNEL_ID` | `your_discord_channel_id_here` | ID Channel Discord untuk notifikasi |

> [!TIP]
> Jika Anda menjalankan n8n di dalam container Docker lokal dan 9Router berjalan di host Windows, gunakan host `http://host.docker.internal:20128/v1` agar container dapat mengakses localhost mesin host.

---

## 🧪 Menguji Webhook n8n

Anda dapat mengirim payload pengujian menggunakan `curl` atau PowerShell:

```bash
curl -X POST http://localhost:5678/webhook/academic-task \
  -H "Content-Type: application/json" \
  -d '{
    "filename": "Tugas_Metode_Penelitian.pdf",
    "text": "Mata Kuliah: Metode Penelitian. Tugas: Buat draft Bab 1 proposal skripsi mengenai AI Agent. Deadline: 2026-12-10 23:59 WIB."
  }'
```
