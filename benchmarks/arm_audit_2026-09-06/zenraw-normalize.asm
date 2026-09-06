
target/release/deps/kernel_tiers-74c320e3f9976eba:	file format mach-o arm64

Disassembly of section __TEXT,__text:

00000001000020b0 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform>:
1000020b0: d10203ff    	sub	sp, sp, #0x80
1000020b4: a9045ff8    	stp	x24, x23, [sp, #0x40]
1000020b8: a90557f6    	stp	x22, x21, [sp, #0x50]
1000020bc: a9064ff4    	stp	x20, x19, [sp, #0x60]
1000020c0: a9077bfd    	stp	x29, x30, [sp, #0x70]
1000020c4: 9101c3fd    	add	x29, sp, #0x70
1000020c8: ad0003e1    	stp	q1, q0, [sp]
1000020cc: aa0103f3    	mov	x19, x1
1000020d0: aa0003f7    	mov	x23, x0
1000020d4: aa0803f4    	mov	x20, x8
1000020d8: d37ef436    	lsl	x22, x1, #2
1000020dc: b40001a1    	cbz	x1, 0x100002110 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x60>
1000020e0: 97fffff3    	bl	0x1000020ac <__RNvCs6rREvFdRhLb_7___rustc35___rust_no_alloc_shim_is_unstable_v2>
1000020e4: aa1603e0    	mov	x0, x22
1000020e8: 52800081    	mov	w1, #0x4                ; =4
1000020ec: 97ffffef    	bl	0x1000020a8 <__RNvCs6rREvFdRhLb_7___rustc19___rust_alloc_zeroed>
1000020f0: b4002560    	cbz	x0, 0x10000259c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4ec>
1000020f4: aa0003f5    	mov	x21, x0
1000020f8: f0000768    	adrp	x8, 0x1000f1000 <__RNvNCNKNvNtNtCs7mRY9FNn263_3std6thread9spawnhook11SPAWN_HOOKS0023___RUST_STD_INTERNAL_VAL$tlv$init>
1000020fc: 9101c108    	add	x8, x8, #0x70
100002100: 39400108    	ldrb	w8, [x8]
100002104: 7100051f    	cmp	w8, #0x1
100002108: 54000101    	b.ne	0x100002128 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x78>
10000210c: 1400006e    	b	0x1000022c4 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x214>
100002110: 52800095    	mov	w21, #0x4               ; =4
100002114: f0000768    	adrp	x8, 0x1000f1000 <__RNvNCNKNvNtNtCs7mRY9FNn263_3std6thread9spawnhook11SPAWN_HOOKS0023___RUST_STD_INTERNAL_VAL$tlv$init>
100002118: 9101c108    	add	x8, x8, #0x70
10000211c: 39400108    	ldrb	w8, [x8]
100002120: 7100051f    	cmp	w8, #0x1
100002124: 54000d00    	b.eq	0x1000022c4 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x214>
100002128: 7100091f    	cmp	w8, #0x2
10000212c: 54000c81    	b.ne	0x1000022bc <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x20c>
100002130: d343fe69    	lsr	x9, x19, #3
100002134: 92400a68    	and	x8, x19, #0x7
100002138: 3dc003e5    	ldr	q5, [sp]
10000213c: b40002a9    	cbz	x9, 0x100002190 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0xe0>
100002140: 3dc007e0    	ldr	q0, [sp, #0x10]
100002144: 4e040400    	dup.4s	v0, v0[0]
100002148: 910042aa    	add	x10, x21, #0x10
10000214c: 910042eb    	add	x11, x23, #0x10
100002150: 6f00e401    	movi.2d	v1, #0000000000000000
100002154: 4f03f602    	fmov.4s	v2, #1.00000000
100002158: ad7f9163    	ldp	q3, q4, [x11, #-0x10]
10000215c: 4ea0d463    	fsub.4s	v3, v3, v0
100002160: 4ea0d484    	fsub.4s	v4, v4, v0
100002164: 4f859063    	fmul.4s	v3, v3, v5[0]
100002168: 4f859084    	fmul.4s	v4, v4, v5[0]
10000216c: 4e21f463    	fmax.4s	v3, v3, v1
100002170: 4e21f484    	fmax.4s	v4, v4, v1
100002174: 4ea2f463    	fmin.4s	v3, v3, v2
100002178: 4ea2f484    	fmin.4s	v4, v4, v2
10000217c: ad3f9143    	stp	q3, q4, [x10, #-0x10]
100002180: 9100814a    	add	x10, x10, #0x20
100002184: 9100816b    	add	x11, x11, #0x20
100002188: f1000529    	subs	x9, x9, #0x1
10000218c: 54fffe61    	b.ne	0x100002158 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0xa8>
100002190: 3dc007e3    	ldr	q3, [sp, #0x10]
100002194: b4001f48    	cbz	x8, 0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
100002198: 927be6c9    	and	x9, x22, #0x7fffffffffffffe0
10000219c: 8b0902ea    	add	x10, x23, x9
1000021a0: 8b0902a9    	add	x9, x21, x9
1000021a4: bd400140    	ldr	s0, [x10]
1000021a8: 1e233800    	fsub	s0, s0, s3
1000021ac: 1e2008a1    	fmul	s1, s5, s0
1000021b0: 1e202028    	fcmp	s1, #0.0
1000021b4: 6f00e400    	movi.2d	v0, #0000000000000000
1000021b8: 1e214c02    	fcsel	s2, s0, s1, mi
1000021bc: 1e2e1001    	fmov	s1, #1.00000000
1000021c0: 1e212040    	fcmp	s2, s1
1000021c4: 1e22cc22    	fcsel	s2, s1, s2, gt
1000021c8: bd000122    	str	s2, [x9]
1000021cc: f100051f    	cmp	x8, #0x1
1000021d0: 54001d60    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
1000021d4: bd400542    	ldr	s2, [x10, #0x4]
1000021d8: 1e233842    	fsub	s2, s2, s3
1000021dc: 1e2208a2    	fmul	s2, s5, s2
1000021e0: 1e202048    	fcmp	s2, #0.0
1000021e4: 1e224c00    	fcsel	s0, s0, s2, mi
1000021e8: 1e212000    	fcmp	s0, s1
1000021ec: 1e20cc20    	fcsel	s0, s1, s0, gt
1000021f0: bd000520    	str	s0, [x9, #0x4]
1000021f4: f100091f    	cmp	x8, #0x2
1000021f8: 54001c20    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
1000021fc: bd400940    	ldr	s0, [x10, #0x8]
100002200: 1e233800    	fsub	s0, s0, s3
100002204: 1e2008a1    	fmul	s1, s5, s0
100002208: 1e202028    	fcmp	s1, #0.0
10000220c: 6f00e400    	movi.2d	v0, #0000000000000000
100002210: 1e214c02    	fcsel	s2, s0, s1, mi
100002214: 1e2e1001    	fmov	s1, #1.00000000
100002218: 1e212040    	fcmp	s2, s1
10000221c: 1e22cc22    	fcsel	s2, s1, s2, gt
100002220: bd000922    	str	s2, [x9, #0x8]
100002224: f1000d1f    	cmp	x8, #0x3
100002228: 54001aa0    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
10000222c: bd400d42    	ldr	s2, [x10, #0xc]
100002230: 1e233842    	fsub	s2, s2, s3
100002234: 1e2208a2    	fmul	s2, s5, s2
100002238: 1e202048    	fcmp	s2, #0.0
10000223c: 1e224c00    	fcsel	s0, s0, s2, mi
100002240: 1e212000    	fcmp	s0, s1
100002244: 1e20cc20    	fcsel	s0, s1, s0, gt
100002248: bd000d20    	str	s0, [x9, #0xc]
10000224c: f100111f    	cmp	x8, #0x4
100002250: 54001960    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
100002254: bd401140    	ldr	s0, [x10, #0x10]
100002258: 1e233800    	fsub	s0, s0, s3
10000225c: 1e2008a1    	fmul	s1, s5, s0
100002260: 1e202028    	fcmp	s1, #0.0
100002264: 6f00e400    	movi.2d	v0, #0000000000000000
100002268: 1e214c02    	fcsel	s2, s0, s1, mi
10000226c: 1e2e1001    	fmov	s1, #1.00000000
100002270: 1e212040    	fcmp	s2, s1
100002274: 1e22cc22    	fcsel	s2, s1, s2, gt
100002278: bd001122    	str	s2, [x9, #0x10]
10000227c: f100151f    	cmp	x8, #0x5
100002280: 540017e0    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
100002284: bd401542    	ldr	s2, [x10, #0x14]
100002288: 1e233842    	fsub	s2, s2, s3
10000228c: 1e2208a2    	fmul	s2, s5, s2
100002290: 1e202048    	fcmp	s2, #0.0
100002294: 1e224c00    	fcsel	s0, s0, s2, mi
100002298: 1e212000    	fcmp	s0, s1
10000229c: 1e20cc20    	fcsel	s0, s1, s0, gt
1000022a0: bd001520    	str	s0, [x9, #0x14]
1000022a4: f100191f    	cmp	x8, #0x6
1000022a8: 540016a0    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
1000022ac: bd401940    	ldr	s0, [x10, #0x18]
1000022b0: 1e233800    	fsub	s0, s0, s3
1000022b4: 1e2008a0    	fmul	s0, s5, s0
1000022b8: 140000aa    	b	0x100002560 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4b0>
1000022bc: 94028d29    	bl	0x1000a5760 <__RNvNtNtNtCs1xa1nU21Rii_8archmage6tokens9generated3arm11neon_detect>
1000022c0: 3707f380    	tbnz	w0, #0x0, 0x100002130 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x80>
1000022c4: d00005a1    	adrp	x1, 0x1000b8000 <GCC_except_table588+0x10>
1000022c8: 91320021    	add	x1, x1, #0xc80
1000022cc: 910083e0    	add	x0, sp, #0x20
1000022d0: 52800402    	mov	w2, #0x20               ; =32
1000022d4: 9402c0b1    	bl	0x1000b2598 <dyld_stub_binder+0x1000b2598>
1000022d8: d343fe69    	lsr	x9, x19, #3
1000022dc: 92400a68    	and	x8, x19, #0x7
1000022e0: 3dc003f6    	ldr	q22, [sp]
1000022e4: b4000aa9    	cbz	x9, 0x100002438 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x388>
1000022e8: ad4107e0    	ldp	q0, q1, [sp, #0x20]
1000022ec: f100113f    	cmp	x9, #0x4
1000022f0: 54000082    	b.hs	0x100002300 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x250>
1000022f4: d280000a    	mov	x10, #0x0               ; =0
1000022f8: 3dc003f6    	ldr	q22, [sp]
1000022fc: 14000037    	b	0x1000023d8 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x328>
100002300: 927edd2a    	and	x10, x9, #0x3fffffffffffffc
100002304: ad400bf6    	ldp	q22, q2, [sp]
100002308: 4e040442    	dup.4s	v2, v2[0]
10000230c: 6f00e403    	movi.2d	v3, #0000000000000000
100002310: 4ea1c424    	fminnm.4s	v4, v1, v1
100002314: aa1703eb    	mov	x11, x23
100002318: 4ea0c405    	fminnm.4s	v5, v0, v0
10000231c: aa1503ec    	mov	x12, x21
100002320: 927edd2d    	and	x13, x9, #0x3fffffffffffffc
100002324: ad431967    	ldp	q7, q6, [x11, #0x60]
100002328: ad424171    	ldp	q17, q16, [x11, #0x40]
10000232c: ad414973    	ldp	q19, q18, [x11, #0x20]
100002330: acc45574    	ldp	q20, q21, [x11], #0x80
100002334: 4ea2d6b5    	fsub.4s	v21, v21, v2
100002338: 4ea2d694    	fsub.4s	v20, v20, v2
10000233c: 4ea2d673    	fsub.4s	v19, v19, v2
100002340: 4ea2d652    	fsub.4s	v18, v18, v2
100002344: 4ea2d631    	fsub.4s	v17, v17, v2
100002348: 4ea2d610    	fsub.4s	v16, v16, v2
10000234c: 4ea2d4e7    	fsub.4s	v7, v7, v2
100002350: 4ea2d4c6    	fsub.4s	v6, v6, v2
100002354: 4f9690c6    	fmul.4s	v6, v6, v22[0]
100002358: 4f9690e7    	fmul.4s	v7, v7, v22[0]
10000235c: 4f969210    	fmul.4s	v16, v16, v22[0]
100002360: 4f969231    	fmul.4s	v17, v17, v22[0]
100002364: 4f969252    	fmul.4s	v18, v18, v22[0]
100002368: 4f969273    	fmul.4s	v19, v19, v22[0]
10000236c: 4f969294    	fmul.4s	v20, v20, v22[0]
100002370: 4f9692b5    	fmul.4s	v21, v21, v22[0]
100002374: 4e23c6b5    	fmaxnm.4s	v21, v21, v3
100002378: 4e23c694    	fmaxnm.4s	v20, v20, v3
10000237c: 4e23c673    	fmaxnm.4s	v19, v19, v3
100002380: 4e23c652    	fmaxnm.4s	v18, v18, v3
100002384: 4e23c631    	fmaxnm.4s	v17, v17, v3
100002388: 4e23c610    	fmaxnm.4s	v16, v16, v3
10000238c: 4e23c4e7    	fmaxnm.4s	v7, v7, v3
100002390: 4e23c4c6    	fmaxnm.4s	v6, v6, v3
100002394: 4ea4c4c6    	fminnm.4s	v6, v6, v4
100002398: 4ea4c610    	fminnm.4s	v16, v16, v4
10000239c: 4ea5c631    	fminnm.4s	v17, v17, v5
1000023a0: 4ea4c652    	fminnm.4s	v18, v18, v4
1000023a4: 4ea5c673    	fminnm.4s	v19, v19, v5
1000023a8: 4ea5c694    	fminnm.4s	v20, v20, v5
1000023ac: 4ea4c6b5    	fminnm.4s	v21, v21, v4
1000023b0: ad005594    	stp	q20, q21, [x12]
1000023b4: ad014993    	stp	q19, q18, [x12, #0x20]
1000023b8: ad024191    	stp	q17, q16, [x12, #0x40]
1000023bc: 4ea5c4e7    	fminnm.4s	v7, v7, v5
1000023c0: ad031987    	stp	q7, q6, [x12, #0x60]
1000023c4: 9102018c    	add	x12, x12, #0x80
1000023c8: f10011ad    	subs	x13, x13, #0x4
1000023cc: 54fffac1    	b.ne	0x100002324 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x274>
1000023d0: eb0a013f    	cmp	x9, x10
1000023d4: 54000320    	b.eq	0x100002438 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x388>
1000023d8: 3dc007e2    	ldr	q2, [sp, #0x10]
1000023dc: 4e040442    	dup.4s	v2, v2[0]
1000023e0: cb090149    	sub	x9, x10, x9
1000023e4: 5280020b    	mov	w11, #0x10              ; =16
1000023e8: aa0a156b    	orr	x11, x11, x10, lsl #5
1000023ec: 8b0b02aa    	add	x10, x21, x11
1000023f0: 8b0b02eb    	add	x11, x23, x11
1000023f4: 6f00e403    	movi.2d	v3, #0000000000000000
1000023f8: 4ea0c400    	fminnm.4s	v0, v0, v0
1000023fc: 4ea1c421    	fminnm.4s	v1, v1, v1
100002400: ad7f9564    	ldp	q4, q5, [x11, #-0x10]
100002404: 4ea2d484    	fsub.4s	v4, v4, v2
100002408: 4f969084    	fmul.4s	v4, v4, v22[0]
10000240c: 4e23c484    	fmaxnm.4s	v4, v4, v3
100002410: 4ea0c484    	fminnm.4s	v4, v4, v0
100002414: 4ea2d4a5    	fsub.4s	v5, v5, v2
100002418: 4f9690a5    	fmul.4s	v5, v5, v22[0]
10000241c: 4e23c4a5    	fmaxnm.4s	v5, v5, v3
100002420: 4ea1c4a5    	fminnm.4s	v5, v5, v1
100002424: ad3f9544    	stp	q4, q5, [x10, #-0x10]
100002428: 9100814a    	add	x10, x10, #0x20
10000242c: 9100816b    	add	x11, x11, #0x20
100002430: b1000529    	adds	x9, x9, #0x1
100002434: 54fffe63    	b.lo	0x100002400 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x350>
100002438: 3dc007e3    	ldr	q3, [sp, #0x10]
10000243c: b4000a08    	cbz	x8, 0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
100002440: 927be6c9    	and	x9, x22, #0x7fffffffffffffe0
100002444: 8b0902ea    	add	x10, x23, x9
100002448: 8b0902a9    	add	x9, x21, x9
10000244c: bd400140    	ldr	s0, [x10]
100002450: 1e233800    	fsub	s0, s0, s3
100002454: 1e200ac1    	fmul	s1, s22, s0
100002458: 1e202028    	fcmp	s1, #0.0
10000245c: 6f00e400    	movi.2d	v0, #0000000000000000
100002460: 1e214c02    	fcsel	s2, s0, s1, mi
100002464: 1e2e1001    	fmov	s1, #1.00000000
100002468: 1e212040    	fcmp	s2, s1
10000246c: 1e22cc22    	fcsel	s2, s1, s2, gt
100002470: bd000122    	str	s2, [x9]
100002474: f100051f    	cmp	x8, #0x1
100002478: 54000820    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
10000247c: bd400542    	ldr	s2, [x10, #0x4]
100002480: 1e233842    	fsub	s2, s2, s3
100002484: 1e220ac2    	fmul	s2, s22, s2
100002488: 1e202048    	fcmp	s2, #0.0
10000248c: 1e224c00    	fcsel	s0, s0, s2, mi
100002490: 1e212000    	fcmp	s0, s1
100002494: 1e20cc20    	fcsel	s0, s1, s0, gt
100002498: bd000520    	str	s0, [x9, #0x4]
10000249c: f100091f    	cmp	x8, #0x2
1000024a0: 540006e0    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
1000024a4: bd400940    	ldr	s0, [x10, #0x8]
1000024a8: 1e233800    	fsub	s0, s0, s3
1000024ac: 1e200ac1    	fmul	s1, s22, s0
1000024b0: 1e202028    	fcmp	s1, #0.0
1000024b4: 6f00e400    	movi.2d	v0, #0000000000000000
1000024b8: 1e214c02    	fcsel	s2, s0, s1, mi
1000024bc: 1e2e1001    	fmov	s1, #1.00000000
1000024c0: 1e212040    	fcmp	s2, s1
1000024c4: 1e22cc22    	fcsel	s2, s1, s2, gt
1000024c8: bd000922    	str	s2, [x9, #0x8]
1000024cc: f1000d1f    	cmp	x8, #0x3
1000024d0: 54000560    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
1000024d4: bd400d42    	ldr	s2, [x10, #0xc]
1000024d8: 1e233842    	fsub	s2, s2, s3
1000024dc: 1e220ac2    	fmul	s2, s22, s2
1000024e0: 1e202048    	fcmp	s2, #0.0
1000024e4: 1e224c00    	fcsel	s0, s0, s2, mi
1000024e8: 1e212000    	fcmp	s0, s1
1000024ec: 1e20cc20    	fcsel	s0, s1, s0, gt
1000024f0: bd000d20    	str	s0, [x9, #0xc]
1000024f4: f100111f    	cmp	x8, #0x4
1000024f8: 54000420    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
1000024fc: bd401140    	ldr	s0, [x10, #0x10]
100002500: 1e233800    	fsub	s0, s0, s3
100002504: 1e200ac1    	fmul	s1, s22, s0
100002508: 1e202028    	fcmp	s1, #0.0
10000250c: 6f00e400    	movi.2d	v0, #0000000000000000
100002510: 1e214c02    	fcsel	s2, s0, s1, mi
100002514: 1e2e1001    	fmov	s1, #1.00000000
100002518: 1e212040    	fcmp	s2, s1
10000251c: 1e22cc22    	fcsel	s2, s1, s2, gt
100002520: bd001122    	str	s2, [x9, #0x10]
100002524: f100151f    	cmp	x8, #0x5
100002528: 540002a0    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
10000252c: bd401542    	ldr	s2, [x10, #0x14]
100002530: 1e233842    	fsub	s2, s2, s3
100002534: 1e220ac2    	fmul	s2, s22, s2
100002538: 1e202048    	fcmp	s2, #0.0
10000253c: 1e224c00    	fcsel	s0, s0, s2, mi
100002540: 1e212000    	fcmp	s0, s1
100002544: 1e20cc20    	fcsel	s0, s1, s0, gt
100002548: bd001520    	str	s0, [x9, #0x14]
10000254c: f100191f    	cmp	x8, #0x6
100002550: 54000160    	b.eq	0x10000257c <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x4cc>
100002554: bd401940    	ldr	s0, [x10, #0x18]
100002558: 1e233800    	fsub	s0, s0, s3
10000255c: 1e200ac0    	fmul	s0, s22, s0
100002560: 1e202008    	fcmp	s0, #0.0
100002564: 6f00e401    	movi.2d	v1, #0000000000000000
100002568: 1e204c20    	fcsel	s0, s1, s0, mi
10000256c: 1e2e1001    	fmov	s1, #1.00000000
100002570: 1e212000    	fcmp	s0, s1
100002574: 1e20cc20    	fcsel	s0, s1, s0, gt
100002578: bd001920    	str	s0, [x9, #0x18]
10000257c: a9005693    	stp	x19, x21, [x20]
100002580: f9000a93    	str	x19, [x20, #0x10]
100002584: a9477bfd    	ldp	x29, x30, [sp, #0x70]
100002588: a9464ff4    	ldp	x20, x19, [sp, #0x60]
10000258c: a94557f6    	ldp	x22, x21, [sp, #0x50]
100002590: a9445ff8    	ldp	x24, x23, [sp, #0x40]
100002594: 910203ff    	add	sp, sp, #0x80
100002598: d65f03c0    	ret
10000259c: 52800080    	mov	w0, #0x4                ; =4
1000025a0: aa1603e1    	mov	x1, x22
1000025a4: 9402bdd4    	bl	0x1000b1cf4 <__RNvNtCs6KVRSXc8uZF_5alloc7raw_vec12handle_error>
1000025a8: aa0003f4    	mov	x20, x0
1000025ac: b40000b3    	cbz	x19, 0x1000025c0 <__RNvNtCshjOmrhwZQBe_6zenraw4simd17normalize_uniform+0x510>
1000025b0: aa1503e0    	mov	x0, x21
1000025b4: aa1603e1    	mov	x1, x22
1000025b8: 52800082    	mov	w2, #0x4                ; =4
1000025bc: 97fffeb9    	bl	0x1000020a0 <__RNvCs6rREvFdRhLb_7___rustc14___rust_dealloc>
1000025c0: aa1403e0    	mov	x0, x20
1000025c4: 9402bf56    	bl	0x1000b231c <dyld_stub_binder+0x1000b231c>
