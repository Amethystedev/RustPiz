//! Sources HLSL des effets à shader (module pur : chaînes ASCII uniquement).
//!
//! Shaders écrits pour RustPiz (licence MIT) d'après la description
//! visuelle des effets ; aucun code d'un autre projet n'est repris.
//!
//! Organisation (voir [`effets_shader`](super::effets_shader)) :
//! - [`VS_HLSL`] : triangle plein écran généré depuis `SV_VertexID` (ni
//!   tampon de sommets ni disposition d'entrée), compilé en `vs_4_0` ;
//! - source d'un pixel shader = [`PRELUDE_HLSL`] + `CORPS_*` +
//!   [`EPILOGUE_HLSL`], compilée en `ps_4_0`, point d'entrée `ps_effet` ;
//! - chaque corps définit `float4 effet(float2 uv, float2 px)` qui renvoie
//!   une couleur en alpha PRÉMULTIPLIÉ et ne dépend que de `uv` / `px`,
//!   `progression` (0 = intacte, 1 = disparue), `graine` et des tailles :
//!   l'ouverture (progression 1 -> 0) est la fermeture exactement inversée.
//!
//! Règles respectées (le HLSL n'est compilé qu'à l'exécution, sous
//! Windows) : uniquement `SampleLevel(..., 0)` (aucun gradient, donc
//! échantillonnage permis dans les branches), boucles bornées `[unroll]`,
//! toutes les variables initialisées sur tous les chemins.

/// Nuanceur de sommets : triangle couvrant tout l'écran.
/// Sommets 0, 1, 2 -> (-1, 1), (3, 1), (-1, -3) en coordonnées normalisées.
pub const VS_HLSL: &str = r#"
float4 vs_plein_ecran(uint id : SV_VertexID) : SV_POSITION
{
    float x = (float)((id << 1) & 2);
    float y = (float)(id & 2);
    return float4(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}
"#;

/// Prélude commun : constantes, image, échantillonneur et utilitaires.
pub const PRELUDE_HLSL: &str = r#"
cbuffer Constantes : register(b0)
{
    float2 taille_cible;   // swapchain size (pixels, margin included)
    float2 taille_image;   // window image size (pixels)
    float2 marge;          // margin (pixels) in X and Y
    float progression;     // 0 = intact, 1 = gone
    float graine;          // random seed, 0 <= graine < 1
    float rayon;           // rounded corner radius (pixels)
    float sens;            // 0 = closing, 1 = opening (information only)
    float opacite;         // global opacity
    float reserve;
};

Texture2D image : register(t0);
SamplerState ech : register(s0);

// Windows 11 rounded corners (image pixels), edge smoothed over 1 pixel.
float masque_coins(float2 px)
{
    float2 q = min(px, taille_image - px);
    float2 d = float2(rayon, rayon) - q;
    float m = 1.0;
    if (rayon > 0.0 && d.x > 0.0 && d.y > 0.0)
    {
        m = saturate(0.5 - (length(d) - rayon));
    }
    return m;
}

// Premultiplied window colour at uv; transparent outside [0, 1].
float4 lire(float2 uv)
{
    float4 c = float4(0.0, 0.0, 0.0, 0.0);
    if (uv.x >= 0.0 && uv.y >= 0.0 && uv.x <= 1.0 && uv.y <= 1.0)
    {
        c = image.SampleLevel(ech, uv, 0.0) * masque_coins(uv * taille_image);
    }
    return c;
}

// Hash of a 2D point, result in 0..1.
float h21(float2 p)
{
    float2 q = frac(p * float2(0.1031, 0.1173) + float2(0.31, 0.77));
    q += dot(q, q.yx + 19.19);
    return frac((q.x + q.y) * q.x);
}

// Smooth value noise in [0, 1].
float bruit(float2 p)
{
    float2 i = floor(p);
    float2 f = frac(p);
    float2 u = f * f * (3.0 - 2.0 * f);
    float a = h21(i);
    float b = h21(i + float2(1.0, 0.0));
    float c = h21(i + float2(0.0, 1.0));
    float d = h21(i + float2(1.0, 1.0));
    return lerp(lerp(a, b, u.x), lerp(c, d, u.x), u.y);
}

// Fractal noise (4 octaves), result in 0..1.
float fbm(float2 p)
{
    float v = 0.0;
    float a = 0.5;
    [unroll] for (int k = 0; k < 4; k++)
    {
        v += a * bruit(p);
        p = p * 2.03 + float2(17.0, 9.0);
        a *= 0.5;
    }
    return v;
}

// Linear remap of x from [a, b] to [0, 1], clamped.
float remap(float x, float a, float b)
{
    return saturate((x - a) / (b - a));
}
"#;

/// Épilogue commun : point d'entrée du pixel shader.
pub const EPILOGUE_HLSL: &str = r#"
float4 ps_effet(float4 pos : SV_POSITION) : SV_TARGET
{
    float2 px = pos.xy - marge;
    return effet(px / taille_image, px) * opacite;
}
"#;

/// Pixellisation : blocs de 1 à 48 px (centre du bloc échantillonné), puis
/// extinction douce des blocs dans un ordre aléatoire (cellules de 48 px).
pub const CORPS_PIXELLISATION: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float taille_bloc = lerp(1.0, 48.0, smoothstep(0.0, 0.8, p));
    float2 bloc = floor(px / taille_bloc);
    float2 centre = (bloc + 0.5) * taille_bloc;
    // Clamped: the centre of an edge block may fall outside the image.
    float4 c = lire(saturate(centre / taille_image));
    // Stable extinction order: hash of the 48 px cell holding the block centre.
    float h = h21(floor(centre / 48.0) + graine * 113.0);
    float seuil = remap(p, 0.4, 1.0) * 1.15;
    float vie = 1.0 - smoothstep(h, h + 0.15, seuil);
    return c * vie;
}
"#;

/// Balayage de pixels : un front traverse la fenêtre (direction choisie par
/// la graine) ; derrière : transparent ; dans la bande de 0,3 : pixellisé
/// (16 px) et partiellement transparent ; devant : intact.
pub const CORPS_BALAYAGE_PIXELS: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float2 bloc = floor(px / 24.0);
    float2 uvb = saturate(((bloc + 0.5) * 24.0) / taille_image);
    float dir = floor(graine * 4.0);
    float pos = uvb.x;
    if (dir > 2.5)
    {
        pos = 1.0 - uvb.y;
    }
    else if (dir > 1.5)
    {
        pos = uvb.y;
    }
    else if (dir > 0.5)
    {
        pos = 1.0 - uvb.x;
    }
    pos = pos * 0.85 + bruit(bloc * 0.35 + graine * 31.0) * 0.15;
    float front = p * 1.3;
    float4 c = float4(0.0, 0.0, 0.0, 0.0);
    if (pos >= front)
    {
        c = lire(uv);
    }
    else if (pos > front - 0.3)
    {
        float a = (pos - (front - 0.3)) / 0.3;
        float2 centre = (floor(px / 16.0) + 0.5) * 16.0;
        c = lire(saturate(centre / taille_image)) * smoothstep(0.0, 1.0, a);
    }
    return c;
}
"#;

/// Glitch : tranches horizontales de hauteur aléatoire renouvelées 12 fois,
/// décalées horizontalement, séparation RVB, blocs vides, puis disparition.
pub const CORPS_GLITCH: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float etape = floor(p * 12.0);
    float g = graine * 53.0 + etape * 7.13;
    // Slices: 40 px bands split once at a random height (8..32 px).
    float bande = floor(px.y / 40.0);
    float coupe = 8.0 + h21(float2(bande, g)) * 24.0;
    float tranche = bande * 2.0 + step(coupe, px.y - bande * 40.0);
    float h1 = h21(float2(tranche, g + 1.7));
    float h2 = h21(float2(tranche, g + 4.3));
    float actif = (h2 < 1.5 * p) ? 1.0 : 0.0;
    float ampleur = max(marge.x, 8.0) * (0.5 + p);
    float decalage = (h1 - 0.5) * 2.0 * ampleur * actif / taille_image.x;
    float sep = 0.01 * p;
    float2 uvd = uv + float2(decalage, 0.0);
    float4 cr = lire(uvd + float2(sep, 0.0));
    float4 cg = lire(uvd);
    float4 cb = lire(uvd - float2(sep, 0.0));
    float4 c = float4(cr.r, cg.g, cb.b, max(max(cr.a, cg.a), cb.a));
    // Empty blocks (64 x 24 px cells).
    float2 cellule = floor(px / float2(64.0, 24.0));
    float vide = (h21(cellule + float2(g * 3.1, g * 1.3)) < 0.35 * p * p) ? 1.0 : 0.0;
    c *= 1.0 - vide;
    return c * (1.0 - smoothstep(0.7, 1.0, p));
}
"#;

/// Roue de pixels : blocs de 16 à 24 px (taille tirée de la graine) éteints
/// dans l'ordre de leur angle autour du centre (balayage d'aiguille), avec
/// une bande pixellisée et assombrie juste avant le front.
pub const CORPS_ROUE_PIXELS: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float taille_bloc = 16.0 + floor(graine * 9.0);
    float2 bloc = floor(px / taille_bloc);
    float2 centre = (bloc + 0.5) * taille_bloc;
    float2 d = centre - taille_image * 0.5;
    // d.x is a multiple of 0.5: the small offset keeps atan2 away from (0, 0).
    float a = frac(atan2(d.y, d.x + 0.001) / 6.2831853 + 0.5 + graine);
    float e = a + 0.1 * h21(bloc + graine * 71.0);
    // e < 1.1 always: the front 1.12 p sweeps every block at p = 1.
    float front = 1.12 * p;
    float bande = 0.15 * saturate(p * 8.0);
    float4 c = lire(uv);
    if (e < front)
    {
        c = float4(0.0, 0.0, 0.0, 0.0);
    }
    else if (e < front + bande)
    {
        float vie = (e - front) / max(bande, 0.0001);
        c = lire(saturate(centre / taille_image)) * (0.4 + 0.6 * vie);
    }
    return c;
}
"#;

/// Fonte (« Doom ») : colonnes de 8 à 16 px qui coulent vers le bas avec un
/// retard lissé entre voisines ; l'image est coupée au bas de la fenêtre.
pub const CORPS_FONTE: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float largeur_col = 8.0 + floor(graine * 9.0);
    float col = floor(px.x / largeur_col);
    // Delay from 0 to 0.35 (excluded): smooth noise between neighbours + jitter.
    float lisse = bruit(float2(col * 0.35, graine * 37.0));
    float delai = 0.35 * (0.75 * lisse + 0.25 * h21(float2(col, graine * 11.0)));
    float q = saturate((p - delai) / 0.65);
    return lire(float2(uv.x, uv.y - 1.1 * q * q));
}
"#;

/// Hexagones : grille hexagonale (rayon max(20 px, min(l, h) / 18)) ; chaque
/// cellule, après un retard aléatoire croissant vers les bords, se rétrécit
/// depuis ses côtés, bordée d'une lueur bleue (intensité 4q(1-q)).
pub const CORPS_HEXAGONES: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float r_hex = max(20.0, min(taille_image.x, taille_image.y) / 18.0);
    float unite = r_hex * 1.7320508;
    float2 g = px / unite;
    // Two offset lattices: the nearest centre gives the (pointy-top) cell.
    float2 pas = float2(1.0, 1.7320508);
    float2 demi = pas * 0.5;
    float2 a = g - pas * floor(g / pas) - demi;
    float2 gb = g - demi;
    float2 b = gb - pas * floor(gb / pas) - demi;
    float2 gv = (dot(a, a) < dot(b, b)) ? a : b;
    float2 id = g - gv;
    float2 ag = abs(gv);
    // Hexagonal distance: 0.5 on the cell edge.
    float hd = max(ag.x, dot(ag, float2(0.5, 0.8660254)));
    // Integer cell key (id.x is a multiple of 0.5, id.y of 0.866).
    float2 cle = floor(id * float2(2.0, 1.1547005) + 0.5);
    float dc = saturate(length(id * unite - taille_image * 0.5) / (0.5 * length(taille_image)));
    float delai = 0.5 * h21(cle + graine * 59.0) + 0.3 * dc;
    // delai < 0.8: every cell is gone at p = 1.
    float q = saturate((p - delai) / 0.2);
    // At q = 0 the limit (0.54) exceeds the edge by >= 1.38 px: cell intact.
    float lim = 0.54 * (1.0 - q);
    float4 w = lire(uv);
    float4 c = w * saturate((lim - hd) * unite);
    float lueur = 4.0 * q * (1.0 - q) * saturate(1.0 - abs(lim - hd) * unite / 3.0);
    float k = lueur * w.a;
    c += float4(float3(0.0, 0.47, 0.83) * k, k);
    return saturate(c);
}
"#;

/// Télévision parasitée : écrasement vertical en ligne puis horizontal en
/// point, lignes décalées au hasard, neige additive, éclaircissement vers le
/// blanc et extinction finale.
pub const CORPS_TELEVISION_PARASITEE: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float sy = lerp(1.0, 0.005, smoothstep(0.0, 0.6, p));
    float sx = lerp(1.0, 0.005, smoothstep(0.6, 1.0, p));
    float tic = floor(p * 30.0);
    float ligne = floor(px.y / 3.0);
    float j = h21(float2(ligne, tic + graine * 91.0)) - 0.5;
    float amp = 0.06 * p * (1.0 - smoothstep(0.55, 0.85, p));
    float2 d = uv - 0.5;
    d.x += j * amp;
    float4 c = lire(d / float2(sx, sy) + 0.5);
    float neige = h21(floor(px) + float2(tic * 17.0, tic * 31.0 + graine * 57.0));
    float ks = 0.5 * smoothstep(0.0, 0.35, p);
    c.rgb = c.rgb + neige * ks * c.a;
    c.rgb = lerp(c.rgb, c.aaa, smoothstep(0.25, 0.8, p));
    return saturate(c) * (1.0 - smoothstep(0.8, 1.0, p));
}
"#;

/// Plané : la fenêtre bascule en arrière autour de l'axe horizontal (60°),
/// rapetisse (70 %), monte de 20 % de sa hauteur et s'efface ; projection
/// perspective inversée exactement (focale 2 max(l, h)).
pub const CORPS_PLANE: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float theta = 1.0471976 * p;
    float st = sin(theta);
    float ct = cos(theta);
    float s = 1.0 - 0.3 * p;
    float f = 2.0 * max(taille_image.x, taille_image.y);
    float2 centre = taille_image * 0.5;
    float X = px.x - centre.x;
    float Y = px.y - centre.y + 0.2 * taille_image.y * p;
    // Top edge recedes: Y = s v cos / (1 - v sin / f), inverted for v.
    float den = s * ct + Y * st / f;
    float4 c = float4(0.0, 0.0, 0.0, 0.0);
    if (den > 0.01)
    {
        float v = Y / den;
        float u = X * (1.0 - v * st / f) / s;
        c = lire((float2(u, v) + centre) / taille_image);
    }
    return c * (1.0 - p);
}
"#;

/// Apparition : tourbillon centré (angle 6p²(1 - r/rmax)), dissolution par
/// bruit fractal à bord doux, couleur tirée vers un blanc spectral.
pub const CORPS_APPARITION: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float2 centre = taille_image * 0.5;
    float2 d = px - centre;
    float rmax = 0.5 * length(taille_image);
    float ang = 6.0 * p * p * saturate(1.0 - length(d) / rmax);
    float sa = sin(ang);
    float ca = cos(ang);
    float2 src = float2(ca * d.x + sa * d.y, ca * d.y - sa * d.x) + centre;
    float2 uvs = src / taille_image;
    float4 c = lire(uvs);
    float n = fbm(uvs * 4.0 + graine * 17.0);
    float seuil = 1.2 * p - 0.1;
    float vis = smoothstep(seuil - 0.05, seuil + 0.05, n);
    float bord = (1.0 - smoothstep(0.0, 0.15, n - seuil)) * saturate(p * 4.0);
    float k = saturate(0.6 * smoothstep(0.0, 0.6, p) + 0.8 * bord);
    c.rgb = lerp(c.rgb, c.a * float3(0.86, 0.93, 1.0), k);
    return c * vis;
}
"#;

/// Énergie A : la fenêtre s'efface pendant que des étincelles dorées
/// (cellules de 6 px, phases aléatoires) scintillent sur sa surface.
pub const CORPS_ENERGIE_A: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float4 w = lire(uv);
    float2 cel = floor(px / 6.0);
    float phi = h21(cel + graine * 13.0) * 6.2831853;
    float sn = max(0.0, sin(phi + 40.0 * p));
    float s4 = (sn * sn) * (sn * sn);
    // sin^20 by multiplications (no pow of 0).
    float etincelle = s4 * s4 * s4 * s4 * s4;
    float rond = 1.0 - smoothstep(0.15, 0.5, length(frac(px / 6.0) - 0.5));
    float env = smoothstep(0.0, 0.3, p) * (1.0 - smoothstep(0.7, 1.0, p));
    float k = etincelle * rond * env * w.a;
    float4 c = w * (1.0 - smoothstep(0.3, 0.8, p));
    c += float4(float3(1.0, 0.8, 0.3) * k, k);
    return saturate(c);
}
"#;

/// Énergie B : même principe en bleu, avec des traînées verticales et une
/// dissolution de la fenêtre qui remonte du bas vers le haut.
pub const CORPS_ENERGIE_B: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float4 w = lire(uv);
    // Dissolution key in [0, 1]: the bottom goes first.
    float cle = (1.0 - uv.y) * 0.75 + 0.25 * bruit(px * 0.05 + graine * 9.0);
    float front = 1.4 * p - 0.2;
    float vis = smoothstep(front - 0.1, front, cle);
    float trainee = bruit(float2(px.x * 0.2, px.y * 0.002 + 3.0 * p));
    float t2 = trainee * trainee;
    float t8 = (t2 * t2) * (t2 * t2);
    float2 cel = floor(px / 6.0);
    float phi = h21(cel + graine * 13.0) * 6.2831853;
    float sn = max(0.0, sin(phi + 40.0 * p));
    float s4 = (sn * sn) * (sn * sn);
    float etincelle = s4 * s4 * s4 * s4 * s4;
    float rond = 1.0 - smoothstep(0.15, 0.5, length(frac(px / 6.0) - 0.5));
    float env = smoothstep(0.0, 0.3, p) * (1.0 - smoothstep(0.7, 1.0, p));
    float k = saturate(1.5 * t8 + 0.6 * etincelle * rond) * env * w.a;
    float4 c = w * vis;
    c += float4(float3(0.4, 0.7, 1.0) * k, k);
    return saturate(c);
}
"#;

/// Feu : un front de combustion irrégulier monte du bas vers le haut ; bande
/// de braises (jaune, orange, rouge, noir) sous le front, fenêtre chauffée
/// juste au-dessus, flammes au-dessus du front jusque dans la marge.
pub const CORPS_FEU: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float4 w = lire(uv);
    // Key (1 - y) + 0.3 n < 1.29; the front 1.6 p - 0.15 reaches 1.45 at p = 1,
    // past the key plus the 0.15 ember band.
    float front = 1.6 * p - 0.15;
    float n = fbm(uv * float2(3.0, 5.0) - float2(0.0, 2.0 * p) + graine * 23.0);
    float d = (1.0 - uv.y) + 0.3 * n - front;
    float intact = step(0.0, d);
    float chaud = 1.0 - smoothstep(0.0, 0.05, d);
    float3 rgb = lerp(w.rgb, w.a * float3(1.0, 0.55, 0.15), 0.7 * chaud);
    float4 c = float4(rgb, w.a) * intact;
    // Embers in [-0.15, 0]: yellow -> orange -> red -> black.
    float t = saturate(-d / 0.15);
    float3 braise = lerp(float3(1.0, 0.85, 0.3), float3(1.0, 0.45, 0.08), saturate(t * 3.0));
    braise = lerp(braise, float3(0.55, 0.06, 0.0), saturate(t * 3.0 - 1.0));
    braise = lerp(braise, float3(0.0, 0.0, 0.0), saturate(t * 3.0 - 2.0));
    float ab = w.a * (1.0 - smoothstep(0.6, 1.0, t)) * (1.0 - intact);
    c += float4(braise * ab, ab);
    // Flames above the front (window and margin).
    float fl = fbm(float2(uv.x * 8.0 + graine * 5.0, uv.y * 4.0 + 4.0 * p));
    float colonne = smoothstep(-0.03, 0.02, uv.x) * smoothstep(-0.03, 0.02, 1.0 - uv.x);
    float env = smoothstep(0.0, 0.08, p) * (1.0 - smoothstep(0.85, 1.0, p));
    float flamme = saturate((fl - max(d, 0.0) * 3.0) * 2.5 - 0.6) * step(-0.02, d) * colonne * env;
    float3 cf = lerp(float3(0.9, 0.15, 0.0), float3(1.0, 0.85, 0.35), flamme);
    float af = flamme * 0.9;
    c = c * (1.0 - af) + float4(cf * af, af);
    return saturate(c);
}
"#;

/// Incinération : combustion partant d'un point tiré de la graine ; bande
/// carbonisée puis bande incandescente devant le front, braises (cellules de
/// 8 px) dérivant vers le haut dans la zone brûlée proche du front.
pub const CORPS_INCINERATION: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float4 w = lire(uv);
    float2 origine = (0.15 + 0.7 * float2(frac(graine * 7.31), frac(graine * 3.17 + 0.5))) * taille_image;
    // Distance normalised by the farthest corner (> 0: origin inside), so
    // dist < 1.235 inside the window; the front 1.5 p - 0.15 reaches 1.35.
    float loin = length(max(origine, taille_image - origine));
    float dist = length(px - origine) / loin + 0.25 * fbm(uv * 6.0 + graine * 11.0);
    float e = dist - (1.5 * p - 0.15);
    float carbone = 1.0 - smoothstep(0.04, 0.12, e);
    float incand = 1.0 - smoothstep(0.0, 0.04, e);
    float vis = smoothstep(-0.015, 0.0, e);
    float3 rgb = lerp(w.rgb, w.a * float3(0.1, 0.05, 0.02), 0.85 * carbone);
    float3 lave = lerp(float3(1.0, 0.4, 0.05), float3(1.0, 0.92, 0.6), incand);
    rgb = lerp(rgb, w.a * lave, incand);
    float4 c = float4(rgb, w.a) * vis;
    // Embers: 8 px cells drifting up uniformly (pattern sampled at uv + (0, 0.3 p)).
    float2 pb = px + float2(0.0, 0.3 * p * taille_image.y);
    float2 cel = floor(pb / 8.0);
    float rond = 1.0 - smoothstep(0.1, 0.35, length(frac(pb / 8.0) - 0.5));
    float grain = rond * step(h21(cel + graine * 29.0), 0.3) * (0.5 + 0.5 * h21(cel + 3.7));
    float zone = (1.0 - step(0.0, e)) * (1.0 - smoothstep(0.05, 0.15, -e));
    float cadre = step(0.0, uv.x) * step(uv.x, 1.0) * step(uv.y, 1.0);
    float envb = smoothstep(0.0, 0.1, p) * (1.0 - smoothstep(0.85, 1.0, p));
    float kb = grain * zone * cadre * envb;
    c += float4(float3(1.0, 0.55, 0.15) * kb, kb);
    return saturate(c);
}
"#;

/// Portail : la fenêtre est aspirée en spirale vers le centre (rotation
/// 10p²/(r + 0.15), réduction 1/(1 + 3p)) puis s'efface ; anneau vert
/// procédural (rayon 0,5 sin(pi p)) d'épaisseur modulée par un bruit
/// tourbillonnant.
pub const CORPS_PORTAIL: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float2 centre = taille_image * 0.5;
    float unite = min(taille_image.x, taille_image.y);
    float2 d = (px - centre) / unite;
    float r = length(d);
    float ang = 10.0 * p * p / (r + 0.15);
    float sa = sin(ang);
    float ca = cos(ang);
    float2 q = float2(ca * d.x - sa * d.y, sa * d.x + ca * d.y) * (1.0 + 3.0 * p);
    float4 c = lire((q * unite + centre) / taille_image) * (1.0 - smoothstep(0.6, 1.0, p));
    // Ring: swirling noise (rotation depends on r and p, no atan2 seam).
    float rr = 0.5 * sin(3.1415927 * p);
    float at = 4.0 * p + 3.0 * r;
    float sb = sin(at);
    float cb = cos(at);
    float2 dt = float2(cb * d.x - sb * d.y, sb * d.x + cb * d.y);
    float ep = 0.02 + 0.05 * fbm(dt * 4.0 + graine * 7.0);
    float x = (r - rr) / ep;
    float anneau = exp(-x * x) * 4.0 * p * (1.0 - p);
    c += float4(float3(0.25, 1.0, 0.45) * anneau, anneau * 0.85);
    return saturate(c);
}
"#;

/// Verre brisé (APPROXIMATION) : éclats de Voronoï (cellules ~ min(l, h)/8)
/// répartis en 6 couches rigides ; chaque couche tourne autour de
/// l'épicentre, glisse et tombe d'un seul bloc (tous ses éclats ensemble),
/// transformation inversée exactement ; fissures éclaircies au début.
pub const CORPS_VERRE_BRISE: &str = r#"
// Voronoi cell (jittered grid points): id of the nearest cell; bord = gap
// between the second and the first distance (0 on a crack).
float2 vb_cellule(float2 g, out float bord)
{
    float2 i = floor(g);
    float d1 = 8.0;
    float d2 = 8.0;
    float2 id = i;
    [unroll] for (int y = -1; y <= 1; y++)
    {
        [unroll] for (int x = -1; x <= 1; x++)
        {
            float2 n = i + float2((float)x, (float)y);
            float2 pt = n + 0.15 + 0.7 * float2(h21(n + graine * 31.0), h21(n + graine * 31.0 + 57.3));
            float2 dv = pt - g;
            float dd = dot(dv, dv);
            if (dd < d1)
            {
                d2 = d1;
                d1 = dd;
                id = n;
            }
            else if (dd < d2)
            {
                d2 = dd;
            }
        }
    }
    bord = sqrt(d2) - sqrt(d1);
    return id;
}

float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float petit = min(taille_image.x, taille_image.y);
    float cote = max(petit / 8.0, 24.0);
    float2 epic = taille_image * (0.3 + 0.4 * float2(frac(graine * 5.3), frac(graine * 9.7 + 0.3)));
    float2 gravite = float2(0.0, 0.5 * taille_image.y);
    float4 c = float4(0.0, 0.0, 0.0, 0.0);
    float trouve = 0.0;
    float bord_trouve = 1.0;
    [unroll] for (int k = 0; k < 6; k++)
    {
        float fk = (float)k;
        float a = 6.2831853 * h21(float2(fk, graine * 41.0));
        float vitesse = (0.15 + 0.25 * h21(float2(fk + 7.0, graine * 13.0))) * petit;
        float omega = (h21(float2(fk + 3.0, graine * 17.0)) - 0.5) * 0.6;
        float2 depl = float2(cos(a), sin(a)) * vitesse * p + gravite * p * p;
        float ang = -omega * p;
        float sa = sin(ang);
        float ca = cos(ang);
        float2 r = px - epic - depl;
        float2 q = epic + float2(ca * r.x - sa * r.y, sa * r.x + ca * r.y);
        float bord = 1.0;
        float2 id = vb_cellule(q / cote, bord);
        float classe = floor(h21(id + graine * 19.0) * 6.0);
        float dedans = step(0.0, q.x) * step(0.0, q.y) * step(q.x, taille_image.x) * step(q.y, taille_image.y);
        // First layer whose shard covers the pixel wins.
        if (trouve < 0.5 && abs(classe - fk) < 0.5 && dedans > 0.5)
        {
            c = lire(q / taille_image);
            bord_trouve = bord;
            trouve = 1.0;
        }
    }
    float fissure = (1.0 - smoothstep(0.0, 0.06, bord_trouve)) * smoothstep(0.0, 0.03, p) * (1.0 - smoothstep(0.06, 0.12, p));
    c.rgb = lerp(c.rgb, c.aaa, 0.8 * fissure);
    return c * (1.0 - smoothstep(0.5, 1.0, p));
}
"#;

/// Désintégration (APPROXIMATION) : chaque pixel part en poussière à un
/// instant t0 (gradient orienté par la graine + bruit) ; la poussière est
/// répartie en 8 couches (cellules de 2 px), chacune dérivant à vitesse
/// constante ; la position source est retrouvée par 2 itérations de point
/// fixe (acceptée si l'écart résiduel est faible), fondu en 0,38.
pub const CORPS_DESINTEGRATION: &str = r#"
// Dust start time of the window pixel s (pixels), from 0 to 0.6 (excluded).
float ds_depart(float2 s)
{
    float2 u = s / taille_image;
    float a = 6.2831853 * graine;
    float2 dir = float2(cos(a), sin(a));
    // |dir.x| + |dir.y| >= 1: no division by zero; lin in [0, 1].
    float lin = saturate(dot(u - 0.5, dir) / (abs(dir.x) + abs(dir.y)) + 0.5);
    float2 v = s / min(taille_image.x, taille_image.y);
    float n = 0.65 * bruit(v * 2.5 + graine * 19.0) + 0.35 * bruit(v * 6.0 + graine * 7.0);
    return 0.6 * (0.7 * lin + 0.3 * n);
}

float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float t0x = ds_depart(px);
    float4 c = float4(0.0, 0.0, 0.0, 0.0);
    if (t0x >= p)
    {
        c = lire(uv);
    }
    float petit = min(taille_image.x, taille_image.y);
    float4 poussiere = float4(0.0, 0.0, 0.0, 0.0);
    [unroll] for (int k = 0; k < 8; k++)
    {
        float fk = (float)k;
        float2 v = float2(0.15 + 0.2 * h21(float2(fk, graine * 23.0)), -(0.1 + 0.25 * h21(float2(fk + 11.0, graine * 29.0)))) * petit;
        float2 s = px - v * max(0.0, p - t0x);
        s = px - v * max(0.0, p - ds_depart(s));
        float age = p - ds_depart(s);
        float classe = floor(h21(floor(s / 2.0) + graine * 37.0) * 8.0);
        float dedans = step(0.0, s.x) * step(0.0, s.y) * step(s.x, taille_image.x) * step(s.y, taille_image.y);
        float2 ecart = s + v * age - px;
        float vie = saturate(1.0 - age / 0.38) * dedans;
        if (age > 0.0 && vie > 0.0 && abs(classe - fk) < 0.5 && dot(ecart, ecart) < 4.0)
        {
            float4 col = lire(s / taille_image);
            float lum = dot(col.rgb, float3(0.3, 0.59, 0.11));
            col.rgb = lerp(col.rgb, lum * float3(0.8, 0.7, 0.6), saturate(age * 2.5));
            poussiere += col * vie;
        }
    }
    c = c + poussiere * (1.0 - c.a);
    return saturate(c);
}
"#;

/// Matrix (APPROXIMATION) : pluie de pseudo-glyphes (motifs 5 x 7 de bits
/// pseudo-aléatoires, renouvelés 20 fois) en colonnes de 12 px ; la fenêtre
/// est cachée au-dessus de la tête de chaque colonne, visible en dessous ;
/// traîne verte exponentielle, tête blanche.
pub const CORPS_MATRIX: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float colonne = floor(px.x / 12.0);
    float delai = 0.6 * h21(float2(colonne, graine * 43.0));
    float yh = (1.6 * p - delai) * taille_image.y;
    float vis = step(yh, px.y);
    float4 c = lire(uv) * vis;
    // Pseudo-glyph: 5 x 7 bits of 2 x 2 px in a 12 x 16 px cell.
    float2 cel = floor(px / float2(12.0, 16.0));
    float2 o = px - cel * float2(12.0, 16.0);
    float2 bit = floor((o - 1.0) / 2.0);
    float dans = step(0.0, bit.x) * step(bit.x, 4.0) * step(0.0, bit.y) * step(bit.y, 6.0);
    float t = floor(20.0 * p + h21(cel + graine * 7.0) * 20.0);
    float allume = step(0.5, h21(float2(cel.x * 7.0 + bit.x + t * 3.1, cel.y * 11.0 + bit.y + t * 1.7)));
    float L = max(64.0, 0.3 * taille_image.y);
    float au_dessus = yh - px.y;
    float traine = exp(-max(au_dessus, 0.0) / L) * (1.0 - vis);
    float tete = 1.0 - smoothstep(0.0, 16.0, au_dessus);
    float env = smoothstep(0.0, 0.05, p) * (1.0 - smoothstep(0.8, 1.0, p));
    float g = allume * dans * traine * env;
    float3 coul = lerp(float3(0.1, 0.95, 0.3), float3(0.85, 1.0, 0.9), tete);
    c += float4(coul * g, g);
    return saturate(c);
}
"#;

/// Feux follets (APPROXIMATION) : dissolution par bruit ; 4 couches de
/// points lumineux, chaque couche translatée d'un bloc (inversible), chaque
/// point oscillant d'au plus 0,4 cellule (voisinage 3 x 3) ; halo gaussien
/// coloré par le pixel d'origine, naissance et mort selon la progression.
pub const CORPS_FEUX_FOLLETS: &str = r#"
float4 effet(float2 uv, float2 px)
{
    float p = saturate(progression);
    float seuil = 1.25 * p - 0.15;
    float4 c = lire(uv) * smoothstep(seuil, seuil + 0.1, fbm(uv * 3.0 + graine * 7.0));
    float petit = min(taille_image.x, taille_image.y);
    float env_g = smoothstep(0.0, 0.05, p) * (1.0 - smoothstep(0.8, 1.0, p));
    float4 lum = float4(0.0, 0.0, 0.0, 0.0);
    [unroll] for (int l = 0; l < 4; l++)
    {
        float fl = (float)l;
        float cs = 28.0 + 8.0 * fl;
        // Uniform translation of the layer, exactly 0 at p = 0.
        float2 tr = float2(sin(p * 3.0 + fl * 1.7) - sin(fl * 1.7), -(1.0 + 0.5 * fl) * p) * 0.12 * petit;
        float2 y = (px - tr) / cs;
        float2 i = floor(y);
        float sigma = 0.18 * cs;
        [unroll] for (int b = -1; b <= 1; b++)
        {
            [unroll] for (int a = -1; a <= 1; a++)
            {
                float2 cel = i + float2((float)a, (float)b);
                float h = h21(cel + fl * 17.0 + graine * 53.0);
                float ph = 6.2831853 * (h * 3.7 + p * 1.5);
                float2 pt = cel + 0.5 + 0.28 * float2(sin(ph), cos(ph * 1.3));
                float2 dv = (y - pt) * cs;
                float halo = exp(-dot(dv, dv) / (sigma * sigma));
                float existe = step(h21(cel * 1.37 + fl * 5.0 + graine * 11.0), 0.45);
                float pre = halo * existe * env_g;
                if (pre > 0.002)
                {
                    // Window pixel the wisp comes from; birth when it dissolves.
                    float2 ouv = (cel + 0.5) * cs / taille_image;
                    float tb = (0.5 * bruit(ouv * 3.0 + graine * 7.0) + 0.37) / 1.25;
                    float vie = smoothstep(tb - 0.05, tb + 0.05, p) * (1.0 - smoothstep(tb + 0.1, tb + 0.35, p));
                    float4 c0 = lire(ouv);
                    float3 coul = saturate(c0.rgb * 1.3 + 0.15 * c0.a);
                    float wgt = pre * vie;
                    lum += float4(coul * wgt, c0.a * wgt);
                }
            }
        }
    }
    return saturate(c + lum);
}
"#;

// Sources passées telles quelles à D3DCompile : ASCII pur exigé.
const _: () = assert!(VS_HLSL.is_ascii());
const _: () = assert!(PRELUDE_HLSL.is_ascii());
const _: () = assert!(EPILOGUE_HLSL.is_ascii());
const _: () = assert!(CORPS_PIXELLISATION.is_ascii());
const _: () = assert!(CORPS_BALAYAGE_PIXELS.is_ascii());
const _: () = assert!(CORPS_GLITCH.is_ascii());
const _: () = assert!(CORPS_ROUE_PIXELS.is_ascii());
const _: () = assert!(CORPS_FONTE.is_ascii());
const _: () = assert!(CORPS_HEXAGONES.is_ascii());
const _: () = assert!(CORPS_TELEVISION_PARASITEE.is_ascii());
const _: () = assert!(CORPS_PLANE.is_ascii());
const _: () = assert!(CORPS_APPARITION.is_ascii());
const _: () = assert!(CORPS_ENERGIE_A.is_ascii());
const _: () = assert!(CORPS_ENERGIE_B.is_ascii());
const _: () = assert!(CORPS_FEU.is_ascii());
const _: () = assert!(CORPS_INCINERATION.is_ascii());
const _: () = assert!(CORPS_PORTAIL.is_ascii());
const _: () = assert!(CORPS_VERRE_BRISE.is_ascii());
const _: () = assert!(CORPS_DESINTEGRATION.is_ascii());
const _: () = assert!(CORPS_MATRIX.is_ascii());
const _: () = assert!(CORPS_FEUX_FOLLETS.is_ascii());
