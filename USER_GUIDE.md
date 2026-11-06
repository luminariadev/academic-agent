# 📖 Panduan Lengkap Pengguna (User Setup Guide)

Panduan langkah-demi-langkah ini dibuat agar **siapa pun pengguna baru** dapat mengkloning repositori ini dan menjalankannya secara mandiri menggunakan **API Key LLM, Bot Discord, dan konfigurasi server milik mereka sendiri**.

---

## 📑 Daftar Isi
1. [Persiapan API Key LLM (Pilih Salah Satu)](#1-persiapan-api-key-llm)
2. [Membuat Bot Discord Pribadi](#2-membuat-bot-discord-pribadi)
3. [Konfigurasi Lingkungan (`.env`)](#3-konfigurasi-lingkungan-env)
4. [Menjalankan Sistem Secara Mandiri](#4-menjalankan-sistem-secara-mandiri)
5. [Impor & Konfigurasi Workflow di n8n](#5-impor--konfigurasi-workflow-di-n8n)
6. [Pengujian Dokumen & Format yang Didukung](#6-pengujian-dokumen--format-yang-didukung)
7. [Troubleshooting & Solusi Masalah Umum](#7-troubleshooting)

---

## 1. Persiapan API Key LLM

Academic Agent menggunakan endpoint standar **OpenAI-compatible**, sehingga Anda bebas memilih penyedia AI mana pun yang Anda sukai:

### Opsi A: Menggunakan 9Router (Rekomendasi - Cepat & Gratis)
1. Jalankan 9Router lokal di komputer Anda pada port `20128`.
2. Gunakan endpoint: `http://127.0.0.1:20128/v1`
3. Gunakan model: `mr-loverman` atau `coding-general`

### Opsi B: Menggunakan OpenAI Resmi (GPT-4o / GPT-4o-mini)
1. Buka [OpenAI Platform](https://platform.openai.com/api-keys) dan buat API Key baru (`sk-...`).
2. Di file `.env`, atur:
   ```ini
   ROUTER_API_BASE=https://api.openai.com/v1
   ROUTER_API_KEY=sk-proj-xxxxxxxxxxxxxxxxxxxxxxxx
   ROUTER_MODEL=gpt-4o-mini
   ```

### Opsi C: Menggunakan OpenRouter (Mendukung Berbagai Model)
1. Buka [OpenRouter.ai](https://openrouter.ai/keys) dan buat API Key.
2. Di file `.env`, atur:
   ```ini
   ROUTER_API_BASE=https://openrouter.ai/api/v1
   ROUTER_API_KEY=sk-or-v1-xxxxxxxxxxxxxxxxxxxxxxx
   ROUTER_MODEL=google/gemini-2.5-flash
   ```

### Opsi D: Menggunakan Ollama (100% Offline / Tanpa Internet)
1. Unduh dan jalankan [Ollama](https://ollama.ai) di komputer Anda.
2. Unduh model: `ollama run qwen2.5:7b`
3. Di file `.env`, atur:
   ```ini
   ROUTER_API_BASE=http://localhost:11434/v1
   ROUTER_API_KEY=ollama
   ROUTER_MODEL=qwen2.5:7b
   ```

---

## 2. Membuat Bot Discord Pribadi

Untuk menerima notifikasi tugas berkancing interaktif di server Discord Anda:

### Langkah 1: Buat Aplikasi di Discord Developer Portal
1. Buka [Discord Developer Portal](https://discord.com/developers/applications).
2. Login dengan akun Discord Anda, lalu klik tombol **New Application** di pojok kanan atas.
3. Beri nama aplikasi Anda (misalnya: `Academic Notifier Bot`), lalu klik **Create**.

### Langkah 2: Ambil Bot Token
1. Di menu sebelah kiri, pilih tab **Bot**.
2. Klik tombol **Reset Token** untuk membuat token baru.
3. Klik **Copy** dan simpan token ini (contoh: `MTU0ND...`). **Jangan bagikan token ini kepada orang lain!**
4. Gulir ke bawah ke bagian **Privileged Gateway Intents**, lalu aktifkan centang:
   - ✅ **Message Content Intent**
5. Klik **Save Changes**.

### Langkah 3: Undang Bot ke Server Discord Anda
1. Di menu sebelah kiri, pilih tab **OAuth2** ➡️ **URL Generator**.
2. Di kotak **Scopes**, centang:
   - ✅ `bot`
3. Di kotak **Bot Permissions** yang muncul di bawahnya, centang:
   - ✅ `Send Messages`
   - ✅ `Embed Links`
   - ✅ `Attach Files`
   - ✅ `Read Message History`
4. Salin URL undangan yang muncul di paling bawah, lalu buka di browser Anda untuk mengundang bot ke server Discord Anda.

### Langkah 4: Dapatkan Channel ID Discord
1. Di aplikasi Discord Anda, buka **User Settings** (ikon gerigi) ➡️ **Advanced** ➡️ aktifkan **Developer Mode**.
2. Masuk ke server Anda, klik kanan channel tempat Anda ingin notifikasi dikirimkan (misal `#tugas` atau `#general`), lalu pilih **Copy Channel ID**.
3. ID ini adalah `DISCORD_CHANNEL_ID` Anda (berupa deretan angka, misal `1544307344584744972`).

---

## 3. Konfigurasi Lingkungan (`.env`)

1. Salin template `.env.example` ke file `.env` di folder utama:
   ```bash
   cp .env.example .env
   ```
2. Buka file `.env` menggunakan teks editor (VS Code, Notepad, atau nano), lalu isi dengan kredensial milik Anda:
   ```ini
   # --- AI Model Provider ---
   ROUTER_API_BASE=http://127.0.0.1:20128/v1
   ROUTER_API_KEY=isi_api_key_anda_disini
   ROUTER_MODEL=mr-loverman

   # --- Discord Bot Integration ---
   DISCORD_BOT_TOKEN=isi_bot_token_discord_anda
   DISCORD_CLIENT_ID=isi_client_id_aplikasi_anda
   DISCORD_CHANNEL_ID=isi_channel_id_tujuan_anda

   # --- Lokasi Folder & Database ---
   WATCH_FOLDER=./tugas_mahasiswa
   CALENDAR_FOLDER=./data/calendar
   DATABASE_URL=sqlite:///./data/tasks.db
   ```

> [!IMPORTANT]
> File `.env` sudah otomatis masuk ke dalam `.gitignore`, sehingga aman dan **tidak akan pernah terunggah ke repositori publik GitHub Anda**.

---

## 4. Menjalankan Sistem Secara Mandiri

### Opsi 1: Mode Folder Watcher Lokal (Paling Sederhana, Tanpa Docker)
Metode ini sangat cocok untuk mahasiswa yang ingin cukup mengunduh tugas dari dosen dan langsung membiarkan AI bekerja:

```bash
# 1. Pastikan dependensi terpasang
pip install -r requirements.txt

# 2. Uji coba koneksi AI dan Discord Anda terlebih dahulu
python -m academic_agent.cli test-ai
python -m academic_agent.cli test-discord

# 3. Jalankan pemantau folder otomatis
python -m academic_agent.cli watch
```

**Cara Penggunaan Sehari-hari:**
Cukup seret (*drag-and-drop*) file `.pdf` atau `.docx` tugas yang diberikan dosen ke dalam folder `tugas_mahasiswa/`.
- Dalam hitungan 5 detik:
  1. AI mengekstrak deadline, sub-task, dan ringkasan.
  2. File kalender `.ics` dibuat di `data/calendar/`.
  3. Tugas dicatat ke SQLite `data/tasks.db`.
  4. Bot Discord mengirimkan pesan embed kaya dengan tombol *Google Calendar* langsung ke server Anda!

---

### Opsi 2: Mode Full Stack Docker (n8n + Python Microservice)
Jika Anda ingin orkestrasi visual canggih n8n:

```bash
# Jalankan seluruh stack n8n dan agent
docker compose up -d
```
- Buka dashboard n8n di: `http://localhost:5678`
- Akses REST API Agent di: `http://localhost:8000/docs`

---

## 5. Impor & Konfigurasi Workflow di n8n

1. Buka n8n di browser (`http://localhost:5678`).
2. Klik **Workflows** ➡️ **Add Workflow** di pojok kanan atas.
3. Klik menu titik tiga (**`⋮`**) di pojok kanan atas kanvas ➡️ pilih **Import from File...**
4. Pilih file [`workflows/academic-agent.n8n.json`](workflows/academic-agent.n8n.json).
5. Pada kanvas n8n:
   - Node **AI Node: 9Router / LLM Inference**: Pastikan URL dan Header `Authorization` diarahkan ke endpoint LLM Anda.
   - Node **Send Discord Interactive Embed**: Pastikan URL channel ID dan Bot Token terisi.
6. Klik **Save** lalu ubah toggle di pojok kanan atas menjadi **Active**.

Untuk menguji webhook n8n:
```bash
curl -X POST http://localhost:5678/webhook/academic-task \
  -H "Content-Type: application/json" \
  -d '{
    "filename": "Tugas_Metode_Penelitian.pdf",
    "text": "Mata Kuliah: Metode Penelitian. Tugas: Proposal Skripsi AI Agent. Deadline: 2026-12-15 23:59 WIB."
  }'
```

---

## 6. Pengujian Dokumen & Format yang Didukung

Repositori ini menyertakan contoh dokumen siap uji di folder `sample_docs/`:
- `sample_docs/tugas_metode_penelitian.pdf`: Format penugasan ilmiah IEEE (PDF).
- `sample_docs/tugas_kecerdasan_buatan.docx`: Silabus & penugasan pemrograman Machine Learning (DOCX).
- `sample_docs/pengumuman_kuis.txt`: Memo singkat kuis perkuliahan (TXT).

Anda dapat menguji pemrosesan dokumen tersebut kapan saja via CLI:
```bash
python -m academic_agent.cli scan sample_docs/tugas_metode_penelitian.pdf
```

Untuk melihat daftar seluruh tugas yang sudah terekam di database:
```bash
python -m academic_agent.cli tasks
```

Untuk menandai tugas nomor 1 selesai dikerjakan:
```bash
python -m academic_agent.cli done 1
```

---

## 7. Troubleshooting

| Gejala Masalah | Penyebab Umum | Solusi |
| :--- | :--- | :--- |
| **Discord HTTP 401 Unauthorized** | Token Bot Discord salah atau kedaluwarsa. | Buka Discord Developer Portal ➡️ Reset Token ➡️ Perbarui `DISCORD_BOT_TOKEN` di `.env`. |
| **Discord HTTP 403 Forbidden** | Bot belum diundang ke server atau tidak memiliki hak kirim pesan. | Periksa izin bot di channel Discord target (pastikan hak `Send Messages` & `Embed Links` aktif). |
| **Discord HTTP 404 Not Found** | Channel ID Discord tidak ditemukan. | Pastikan `DISCORD_CHANNEL_ID` sesuai dengan channel tujuan di server tempat bot berada. |
| **Gagal terhubung ke AI (Connection Refused)** | 9Router / Ollama lokal belum dinyalakan. | Pastikan 9Router atau Ollama berjalan di latar belakang sebelum menjalankan agent. |
| **Karakter Unicode error di Terminal Windows** | Konsol cmd/PowerShell belum menggunakan UTF-8. | Academic Agent sudah otomatis mengaktifkan UTF-8 reconfigure. Atau jalankan di terminal modern seperti Windows Terminal. |

---

<div align="center">
  <sub>Dokumentasi Resmi Academic Agent • Dikelola oleh <a href="https://github.com/luminariadev">luminariadev</a></sub>
</div>
