# BH Escape Reference Audit

> Completed: April 7, 2026. All pages in the XML sitemap were visited.
> This is the primary reference document — ALL implementation tasks build against this.

---

## Site Structure & Navigation

### Sitemap (All URLs)
```
https://bhescape.com.br/                        — Homepage
https://bhescape.com.br/santo-antonio/          — Unit: Santo Antônio
https://bhescape.com.br/bh-del-rey/            — Unit: Del Rey
https://bhescape.com.br/betim/                  — Unit: Betim
https://bhescape.com.br/contagem/               — Unit: Contagem (Shopping)
https://bhescape.com.br/reservas-bh-del-rey/   — Del Rey Reservations (holding page)
https://bhescape.com.br/aniversarios/           — Birthdays Page
https://bhescape.com.br/eventos/                — Events Page (mirrors Birthdays content)
https://bhescape.com.br/empresarial/            — Corporate / B2B Page
https://bhescape.com.br/contato/               — Contact Page
https://bhescape.com.br/salas/                  — All Rooms (aggregated listing)
https://bhescape.com.br/reservas/               — Reservations Gateway
https://bhescape.com.br/bhescapereservas/       — Reservations Hub (unit selector)
https://bhescape.com.br/cidades/               — Cities Page (unit gateway)
https://bhescape.com.br/orcamentos/            — Quote Request Page
https://bhescape.com.br/obrigado-reservas/     — Thank You / Post-Booking Confirmation
https://bhescape.com.br/politica-de-privacidade/ — Privacy Policy
https://bhescape.com.br/termos-de-servico/     — Terms of Service
https://bhescape.com.br/home                   — Alias → Homepage
```

### Primary Navigation (Header)
| Label | Destination |
|---|---|
| Início | / |
| Unidades → Santo Antônio | /santo-antonio/ |
| Unidades → Del Rey | /bh-del-rey/ |
| Unidades → Betim | /betim/ |
| Unidades → Contagem | /contagem/ |
| Aniversários | /aniversarios/ |
| Empresarial | /empresarial/ |
| Contato | /contato/ |
| **Reservar agora** (CTA button) | Elementor off-canvas popup |

### Footer Navigation
- Mirrors primary nav
- Social media: Instagram (`instagram.com/bhescape`), Facebook (`facebook.com/bhescape`), LinkedIn (`linkedin.com/company/bhescape`)
- Legal: Privacy Policy, Terms of Service
- Copyright: © 2025 BH Escape. Todos os Direitos Reservados.

---

## Visual Language

### Colour Palette

| Role | Hex | Notes |
|---|---|---|
| Brand Gold / Accent | `#F3B71B` | Primary accent; used on CTAs, borders, highlights, dividers |
| Brand Gold (hover/dark) | `#C38D00` | Darker gold for button hover states |
| Brand Gold (transparent) | `#F3B71B70` | Overlay/divider use |
| Near-Black / Body BG | `#0A0A0A` | Site background; very dark near-black |
| Pure Black | `#000000` | Text |
| White | `#FFFFFF` | Text on dark, section backgrounds |
| Scroll Button Purple | `#5636d1` | Scroll-to-top button |
| Scroll Button Hover Pink | `#e2498a` | Hover state on scroll-to-top |

**Brand feel:** Dark, moody, thriller/adventure aesthetic with gold accents. High contrast. Noir game aesthetic.

### Typography

| Role | Font Family | Format |
|---|---|---|
| Headings / Display | **Bebas Neue** | Regular, condensed sans-serif; self-hosted |
| Body / UI | **DM Sans** | Regular and Bold (18pt, 24pt); self-hosted |
| Fallback | `sans-serif` | Generic |

**Font sizes:** Small 13px · Medium 20px · Large 36px · X-Large 42px

### Photography Style
- Dark, dramatic, cinematic escape room settings
- Room "cover" photos (`CAPA-EMBOSCADA.webp`, `CAPA-MUNDO-INVERTIDO.webp`, `CAPA-TVIRUS.webp`, `CAPA-WANDINHA.webp`) — theatrical, moody
- Location exterior/interior shots for each unit
- Corporate clients shown as monochrome or styled logo grid
- Testimonials are **image-based** (not text), embedded as `.webp` files

### Background Treatments
- Full-bleed hero background images per unit with dark overlay at **~63% opacity**
- Decorative "torn paper" / "cut paper" border dividers between sections:
  - `BORDA-PAPEL-CORTADO-AMARELO-TOP-.webp` — yellow cut-paper edge (top)
  - `BORDA-PAPEL-CORTADO-AMARELO-BOTTOM.webp` — yellow cut-paper edge (bottom)
  - `BORDA-PAPEL-CORTADO-1.webp` / `BORDA-PAPEL-CORTADO-2.webp` — neutral variants
- Texture bands as section separators: `FAIXA-TOP.webp`, `faixa_3.webp`, `faixa_4.webp`
- Border radius: `80px`, `20px`, `500px` used on various UI elements

### Brand Assets
- Primary logo (yellow): `LOGO-AMARELA.webp`
- Navigation logo: `LOGO-MENU.webp`
- Favicon: `FAVICON.png`
- Decorative hand/claw graphic: `MAO.webp` (section accent)

---

## Page-by-Page Breakdown

---

### Homepage (`/`)

**Page Title:** BH Escape — Escape Rooms em BH, Betim e Contagem

#### Section 1: Hero
- **Background:** `DKTP_HERO_HOME.webp` (desktop) / `IMG_HOME_MOBILE.webp` (mobile)
- **Headline:** "Entre na sala. Resolva os enigmas. Escape com sua equipe."
- **Subheadline:** "Escape Rooms imersivos e divertidos para amigos, famílias, aniversários e empresas."
- **CTA:** "Reservar agora" → triggers Elementor off-canvas panel

#### Section 2: Statistics Bar
| Stat | Value |
|---|---|
| Jogadores | +41.000 |
| Empresas atendidas | +700 |
| Salas temáticas exclusivas | 13 |

#### Section 3: What Is an Escape Room?
- **Heading:** "É como um jogo… só que de verdade."
- **Body:** "Você e seu grupo entram em uma sala temática cheia de enigmas. O objetivo? Resolver tudo em até 60 minutos para escapar no tempo certo. É diversão com trabalho em equipe, raciocínio e adrenalina."
- **Media:** Embedded YouTube video (lazy-loaded)

#### Section 4: Corporate Clients Logo Grid
- **Heading:** "Empresas que já jogaram com a gente"
- **17 logos:** Banco do Brasil, Coca-Cola, Google, Heineken, Hotmart, Banco Inter, Itaú, Localiza, Magazine Luiza (Magalu), MRV, Petrobras, Grupo Sada, Sebrae, Sesc, Sólides, Suzano, TOTVS
- Logo images: 150×150px `.webp` files

#### Section 5: How It Works (4 steps)
1. **01** — Escolha sua unidade
2. **02** — Veja as salas disponíveis
3. **03** — Reserve o melhor horário
4. **04** — Venha viver a experiência

#### Section 6: Unit Cards (4 locations)
Each card: unit photo + unit name + 2 CTAs ("Reservar agora" → Bookeo, "Saber mais" → unit page)

| Unit | Image |
|---|---|
| Santo Antônio | `IMG_SANTO_ANTONIO.webp` |
| Del Rey | `DEL-REY-CABINE.webp` |
| Betim | `BETIM.webp` |
| Contagem | `CAPA-CONTAGEM1.webp` |

#### Section 7: For Whom?
Three audience segments with image, heading, short description:
- **Aniversários** — "Festa diferente, segura e cheia de adrenalina"
- **Família e amigos** — "Diversão fora das telas, juntos de verdade"
- **Empresas** — "Integração, estratégia e desafios pensados para equipes"
- Background: `IMG-PRA-QUEM.webp`

#### Section 8: Testimonials / Cases de Sucesso
**Heading:** "Depoimentos Reais / Cases de Sucesso"
Five corporate cases as image cards (desktop + mobile `.webp` pairs):
- MRV, Grupo Sada, Grupo Cornélio, Colégio Santo Agostinho, Alob Sports
- ⚠️ Testimonial text is **baked into images** — not selectable HTML text

#### Section 9: About Us
- **Heading:** "Quem Somos"
- **Body:** "Desde 2017, o BH Escape cria experiências imersivas e desafiadoras que vão muito além de um simples jogo..."
- **Award:** Prêmio Galo de Gramado (branded game for Cornélio Brenand)
- **Tagline:** "Mais que jogos, criamos histórias para viver, juntos."
- **Image:** `IMG-SOBRE-BH-ESCAPE.webp`

#### Section 10: Final CTA
- **Headline:** "BH Escape – onde cada enigma conta uma história."
- **Subheadline:** "Pronto para viver essa experiência?"
- **CTA:** "Reservar agora"

#### Section 11: FAQ (6 questions)
| Question | Answer |
|---|---|
| Posso tirar dúvidas por WhatsApp? | Link to WhatsApp |
| Quanto tempo dura a experiência? | Até 60 min; chegar 15 min antes |
| Quantas pessoas podem jogar? | 2–10 jogadores; ideal 4–6 |
| Tem idade mínima? | A partir de 8 anos com responsável |
| Onde ficam as unidades? | BH (Santo Antônio e Del Rey), Betim e Contagem |
| Como faço para reservar? | Clique em "Reservar agora", escolha unidade e horário |

---

### Unit: Santo Antônio (`/santo-antonio/`)

**Hero:** "Unidade Santo Antônio" — "A unidade mais completa de BH."
**Background:** `BG-STO-ANTONIO.webp` with dark overlay
**Address:** Rua Cristina, 1445 – Santo Antônio, Belo Horizonte
**Hours:** Todos os dias, das 10h às 22h
**Capacity:** Até 45 pessoas em eventos
**WhatsApp:** (31) 98362-6317

**Rooms (6):**

| Room | Difficulty | Players | Price | Bookeo Type ID |
|---|---|---|---|---|
| EMBOSCADA | Difícil | 3–10 | R$80/pp | `41577L7746U190DC2860F9` |
| A Maldição Pirata | Difícil | 3–8 | R$80/pp | `41577NXRAEJ183B2C4A1B0` |
| Mundo Invertido | Intermediário | 3–8 | R$80/pp | `41577FCL6LC17F090C5E3D` |
| O Mistério de Santos Dumont | Intermediário | 3–10 | R$80/pp | `41577979K3L174B2080B6B` |
| T-Vírus | Muito Difícil | 3–10 | R$80/pp | `41577UPKEMN189D165CE87` |
| WANDINHA | Intermediário | 3–8 | R$80/pp | `41577XTATK418CAC35D141` |

**Bookeo account:** `bookeo.com/bhescape`

---

### Unit: Del Rey (`/bh-del-rey/`)

**Format:** Cabines de jogos (gaming cabins) — NOT traditional escape rooms
**Address:** Shopping Del Rey, Av. Presidente Carlos Luz, 3001 – Pampulha
**Booking:** First-come, first-served (NO online reservation)
**Payment:** Credit card or Pix on-site
**Phone:** (31) 98965-5911

**Rooms (2 cabins, 20 min, R$40 total):**

| Room | Description |
|---|---|
| Cápsula 27 | Missão espacial — reativar pod de fuga antes do oxigênio acabar |
| Umbrella Corp - Vírus Mortal | Vírus liberado — descobrir a cura antes da mutação |

**Min age:** 8 years (both rooms)

---

### Unit: Betim (`/betim/`)

**Address:** Av. Juiz Marco Túlio Isaac, 1119 – Ingá Alto, Betim (Shopping Monte Carmo)
**Hours:** Sexta e sábado 13h–22h / Domingo 14h–20h
**Phone:** (31) 3157-2365
**Capacity:** Até 20 pessoas em eventos
**Bookeo account:** `bookeo.com/bhescapebtm` ⚠️ DIFFERENT from main account

**Rooms (3):**

| Room | Difficulty | Players | Price | Bookeo Type ID |
|---|---|---|---|---|
| A Última Charada | Intermediário | 2–6 | R$80/pp | `41590FWF7TT191FBB255D2` |
| Escape Mortal | Muito Difícil | 2–6 | R$80/pp | `41590WEMYNT19243557A0A` |
| Excalibur | Difícil | 2–6 | R$80/pp | `41590K7P9E619243593359` |

---

### Unit: Contagem (`/contagem/`)

**Address:** Av. Severino Ballesteros, 850 – Cabral, Contagem (Shopping Contagem, Andar L1)
**Hours:** Todos os dias, das 10h às 22h
**Phone:** (31) 99691-0272
**Bookeo account:** `bookeo.com/bhescape` (same as Santo Antônio)

**Rooms (3):**

| Room | Difficulty | Players | Price | Bookeo Type ID | Notes |
|---|---|---|---|---|---|
| Canibal Americano | Intermediário | 2–5 | R$80/pp | `41577KUC7M919B991E0EE7` | Min age 13 with parental consent |
| Samhain: Halloween Eterno | Difícil | 2–5 | R$80/pp | `41577PCF7R719B99205E87` | |
| Mistérios S/A: O Fantasma da Mansão Maltravers | Intermediário | Up to 6 | R$80/pp | `41577NMKHHH19B99234209` | |

---

### Birthdays Page (`/aniversarios/`)

**Headline:** "Uma festa diferente, divertida e cheia de mistério!"
**Primary CTA:** "Fale conosco no WhatsApp" → `wa.me/5531983626317`
- Rooms with child adaptations (from age 8)
- Team trained for children/teenagers
- Space for photos and celebration post-game
- Note: `/eventos/` mirrors this page

---

### Corporate Page (`/empresarial/`)

**Headline:** "Desenvolva sua equipe com experiências imersivas de escape"
**4 Modalities:**

| Modality | Location | Capacity | Duration |
|---|---|---|---|
| Presencial – Santo Antônio | Rua Cristina, 1445 | Up to 45 people | 60 min |
| Presencial – Betim | Shopping Monte Carmo | Up to 20 people | 60 min |
| In Company – Ameaça Nuclear | Client's location | Up to 300 simultaneous | 60–120 min |
| Online – Live Escape Game | 100% virtual | Variable | Variable |

**Primary CTA:** "Fale conosco no WhatsApp"

---

### Contact Page (`/contato/`)

**Form fields:** Nome, Email, Assunto, Mensagem + "Enviar"
**Email:** [email protected]

| Unit | Address | Phone |
|---|---|---|
| Santo Antônio | Rua Cristina, 1445 | (31) 98362-6317 |
| Del Rey | Av. Presidente Carlos Luz, 3001 – Pampulha | (31) 98965-5911 |
| Betim | Av. Juiz Marco Túlio Isaac, 1119 – Ingá Alto | (31) 3157-2365 |
| Contagem | Av. Severino Ballesteros, 850 – Cabral, L1 | (31) 99691-0272 |

---

## Booking Flow

### Standard Units (Santo Antônio, Contagem, Betim)

1. User clicks "Reservar agora" or room-specific "Reservar" button
2. Room card shows: cover photo, name, difficulty badge, player count, price, CTA
3. Redirects to Bookeo: `www-1577h.bookeo.com` (main) or `www-1590g.bookeo.com` (Betim)
4. Bookeo flow: date picker → available time slots → group size → participant info → payment
5. Confirmation: redirected to `/obrigado-reservas/`

### Del Rey (Cabines)
- No online booking
- Walk-in only, first-come first-served
- Payment: credit card or Pix on-site

### Homepage Booking Trigger
- Header "Reservar agora" → Elementor off-canvas panel slides in
- Shows unit selector → routes to Bookeo per unit

---

## Component Library

### 1. Navigation Header
- Logo left: `LOGO-MENU.webp`
- Dropdown for "Unidades"
- CTA button right: "Reservar agora" (opens off-canvas)
- Dark/black background, sticky on scroll

### 2. Hero Section
- Full-width background image + dark overlay (~63% opacity)
- Centered: H1 headline + subheadline + single CTA button
- Mobile uses separate image asset

### 3. Stats Bar
- 3 statistics side by side
- Large number in **Bebas Neue** (gold `#F3B71B`) + label in **DM Sans**

### 4. Room Card
- Cover image (full-width)
- Room name (Bebas Neue)
- Difficulty badge
- Player count range
- Price (R$ 80 por pessoa)
- "Reservar" CTA → Bookeo deep link with `type=` param
- Dark background, rounded corners

### 5. Unit Card
- Unit photo + name + 2 CTAs ("Reservar agora" + "Saber mais")

### 6. How It Works (4-Step Process)
- Numbered labels (01–04) + icon + short description
- Horizontal row, dark background section

### 7. Corporate Logo Grid / Carousel
- 17 client logos, 150×150px `.webp`, displayed in grid or scrolling carousel

### 8. Testimonial Image Cards
- Full testimonial as `.webp` image (NOT HTML text)
- Desktop + mobile variants per testimonial
- Displayed in carousel/slider

### 9. Audience Segment Cards
- 3 cards: Aniversários / Família e amigos / Empresas
- Image + heading + short description

### 10. Final CTA Section
- Dark background with gold paper-cut border dividers
- Centered headline + subheadline + large "Reservar agora"

### 11. FAQ Accordion
- Question → expandable answer
- 6 items on homepage
- Dark styling

### 12. Section Dividers (Paper-Cut Borders)
- Decorative `.webp` strips between sections
- Yellow: `BORDA-PAPEL-CORTADO-AMARELO-TOP-.webp` / `BOTTOM.webp`
- Neutral: `BORDA-PAPEL-CORTADO-1.webp` / `BORDA-PAPEL-CORTADO-2.webp`
- Texture faixas: `FAIXA-TOP.webp`, `faixa_3.webp`, `faixa_4.webp`

### 13. Footer
- Dark `#0A0A0A` background
- Multi-column: Logo + tagline | Navigation | Contact per unit | Social
- Legal row: Privacy Policy · Terms of Service
- © 2025 BH Escape. Todos os Direitos Reservados.

### 14. WhatsApp Float Button
- Persistent floating CTA
- `https://api.whatsapp.com/send/?phone=5531983626317`

### 15. Contact Form
- Fields: Nome, Email, Assunto, Mensagem
- Submit: "Enviar"

### 16. Booking Off-Canvas Panel
- Triggered by header CTA
- Slides in from side
- Unit selector → Bookeo routing

---

## Key Content & Copy

### Statistics / Trust Signals
- **+41.000 jogadores**
- **+700 empresas atendidas**
- **13 salas temáticas exclusivas**
- **Desde 2017**
- **Prêmio Galo de Gramado** (branded game for Cornélio Brenand)

### Key CTAs (in order of prominence)
- **"Reservar agora"** — Primary booking CTA
- **"Fale conosco no WhatsApp"** — Secondary (birthdays, corporate, quotes)
- **"Saber mais"** — Unit discovery
- **"Ver salas"** — Room discovery (Del Rey on homepage)

### Brand Taglines
- "Entre na sala. Resolva os enigmas. Escape com sua equipe." — Hero
- "É como um jogo… só que de verdade." — Explainer hook
- "BH Escape – onde cada enigma conta uma história." — Brand line
- "Mais que jogos, criamos histórias para viver, juntos." — About Us

### Pricing
| Unit | Price |
|---|---|
| Santo Antônio | R$80/person |
| Contagem | R$80/person |
| Betim | R$80/person |
| Del Rey (cabins) | R$40 total (solo or pair) |

### Difficulty System
- Intermediário · Difícil · Muito Difícil

### Age Restrictions
- General: 8+ with guardian
- Canibal Americano (Contagem): 13+ with parental consent
- Del Rey cabins: 8+

### WhatsApp Numbers
| Unit | Number |
|---|---|
| Santo Antônio / General | (31) 98362-6317 |
| Del Rey | (31) 98965-5911 |
| Betim | (31) 3157-2365 |
| Contagem | (31) 99691-0272 |

### Social Media
- Instagram: `@bhescape`
- Facebook: `facebook.com/bhescape`
- LinkedIn: `linkedin.com/company/bhescape`

### Technology Stack (original site)
- CMS: WordPress + Elementor Pro
- Booking: Bookeo (accounts: `bhescape` + `bhescapebtm`)
- Fonts: Bebas Neue + DM Sans (self-hosted)
- Images: `.webp` throughout
- Video: YouTube embed (lazy-loaded via WP Rocket)
- Contact: WhatsApp API + contact form
