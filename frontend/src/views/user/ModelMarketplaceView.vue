<template>
  <AppLayout>
    <div class="flex min-h-[calc(100vh-4rem)] flex-col bg-white dark:bg-dark-900 lg:flex-row">
      <aside class="w-full flex-shrink-0 border-b border-gray-200 bg-gray-50/70 p-5 dark:border-dark-700 dark:bg-dark-900 lg:w-72 lg:border-b-0 lg:border-r">
        <div class="mb-5 flex items-center justify-between">
          <h2 class="text-base font-semibold text-gray-900 dark:text-white">{{ t('modelMarketplace.filters') }}</h2>
          <button class="text-sm font-medium text-primary-600 hover:text-primary-700 dark:text-primary-400" @click="resetFilters">
            {{ t('modelMarketplace.reset') }}
          </button>
        </div>

        <section>
          <h3 class="mb-3 text-xs font-semibold uppercase text-gray-500 dark:text-gray-400">{{ t('modelMarketplace.providers') }}</h3>
          <div class="flex flex-wrap gap-2">
            <button
              v-for="provider in providers"
              :key="provider.value"
              class="inline-flex min-h-9 items-center gap-2 rounded-md border px-3 text-sm transition-colors"
              :class="selectedProvider === provider.value
                ? 'border-primary-500 bg-primary-50 text-primary-700 dark:bg-primary-900/20 dark:text-primary-300'
                : 'border-gray-200 bg-white text-gray-600 hover:border-gray-300 dark:border-dark-700 dark:bg-dark-800 dark:text-gray-300'"
              @click="selectedProvider = provider.value"
            >
              <PlatformIcon v-if="provider.value !== 'all'" :platform="provider.value as GroupPlatform" size="xs" />
              <Icon v-else name="grid" size="xs" />
              {{ provider.label }}
              <span class="rounded bg-gray-100 px-1.5 py-0.5 text-xs text-gray-500 dark:bg-dark-700 dark:text-gray-300">{{ provider.count }}</span>
            </button>
          </div>
        </section>

        <section class="mt-7">
          <h3 class="mb-3 text-xs font-semibold uppercase text-gray-500 dark:text-gray-400">{{ t('modelMarketplace.groups') }}</h3>
          <div class="max-h-[48vh] space-y-1.5 overflow-y-auto pr-1">
            <button
              v-for="group in groups"
              :key="group.id"
              class="flex min-h-10 w-full items-center justify-between rounded-md border px-3 text-left text-sm transition-colors"
              :class="selectedGroup === group.id
                ? 'border-primary-500 bg-primary-50 font-medium text-primary-700 dark:bg-primary-900/20 dark:text-primary-300'
                : 'border-gray-200 bg-white text-gray-700 hover:border-gray-300 dark:border-dark-700 dark:bg-dark-800 dark:text-gray-300'"
              @click="selectedGroup = group.id"
            >
              <span class="min-w-0 truncate">{{ group.name }}</span>
              <span class="ml-2 rounded bg-gray-100 px-1.5 py-0.5 text-xs text-gray-500 dark:bg-dark-700 dark:text-gray-300">{{ group.count }}</span>
            </button>
          </div>
        </section>
      </aside>

      <main class="min-w-0 flex-1 p-5 md:p-7">
        <header class="border-b border-gray-200 pb-5 dark:border-dark-700">
          <div class="flex flex-col justify-between gap-4 md:flex-row md:items-end">
            <div>
              <div class="mb-2 flex items-center gap-3">
                <h1 class="text-2xl font-semibold text-gray-950 dark:text-white">{{ t('modelMarketplace.title') }}</h1>
                <span class="rounded-md bg-primary-50 px-2 py-1 text-xs font-medium text-primary-700 dark:bg-primary-900/20 dark:text-primary-300">
                  {{ t('modelMarketplace.modelCount', { count: filteredModels.length }) }}
                </span>
              </div>
              <p class="text-sm text-gray-500 dark:text-gray-400">{{ t('modelMarketplace.description') }}</p>
            </div>
            <button class="btn btn-secondary btn-icon" :title="t('common.refresh')" :disabled="loading" @click="loadData">
              <Icon name="refresh" size="md" :class="loading && 'animate-spin'" />
            </button>
          </div>
        </header>

        <div class="my-5 flex flex-col gap-3 sm:flex-row">
          <div class="relative flex-1">
            <Icon name="search" size="md" class="absolute left-3 top-1/2 -translate-y-1/2 text-gray-400" />
            <input v-model="query" class="input pl-10" :placeholder="t('modelMarketplace.searchPlaceholder')" />
          </div>
          <div class="inline-flex h-10 flex-shrink-0 rounded-md border border-gray-200 p-1 dark:border-dark-700">
            <button class="btn-icon h-8 w-8" :class="viewMode === 'grid' ? 'bg-primary-50 text-primary-600 dark:bg-primary-900/20' : 'text-gray-400'" :title="t('modelMarketplace.gridView')" @click="viewMode = 'grid'">
              <Icon name="grid" size="sm" />
            </button>
            <button class="btn-icon h-8 w-8" :class="viewMode === 'table' ? 'bg-primary-50 text-primary-600 dark:bg-primary-900/20' : 'text-gray-400'" :title="t('modelMarketplace.tableView')" @click="viewMode = 'table'">
              <Icon name="chart" size="sm" />
            </button>
          </div>
        </div>

        <div v-if="loading" class="flex min-h-64 items-center justify-center">
          <Icon name="refresh" size="lg" class="animate-spin text-primary-500" />
        </div>
        <div v-else-if="filteredModels.length === 0" class="flex min-h-64 flex-col items-center justify-center border border-dashed border-gray-200 text-center dark:border-dark-700">
          <Icon name="inbox" size="xl" class="mb-3 text-gray-300" />
          <p class="text-sm text-gray-500">{{ t('modelMarketplace.empty') }}</p>
        </div>

        <div v-else-if="viewMode === 'grid'" class="grid grid-cols-1 gap-3 xl:grid-cols-2 2xl:grid-cols-3">
          <article v-for="model in filteredModels" :key="model.key" class="relative min-h-40 border border-gray-200 bg-white p-4 transition-colors hover:border-primary-300 dark:border-dark-700 dark:bg-dark-800 dark:hover:border-primary-700">
            <button class="btn-ghost btn-icon absolute right-3 top-3" :title="t('modelMarketplace.copyModel')" @click="copyModel(model.name)">
              <Icon :name="copiedModel === model.name ? 'check' : 'copy'" size="sm" />
            </button>
            <div class="flex items-start gap-3 pr-9">
              <div class="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-md bg-gray-100 text-gray-700 dark:bg-dark-700 dark:text-gray-200">
                <PlatformIcon :platform="model.platform as GroupPlatform" size="md" />
              </div>
              <div class="min-w-0">
                <h2 class="break-all text-base font-semibold text-gray-950 dark:text-white">{{ model.name }}</h2>
                <p class="mt-1 text-xs text-gray-500">{{ priceSummary(model) }}</p>
              </div>
            </div>
            <div class="mt-4 flex flex-wrap gap-1.5">
              <span class="rounded-md px-2 py-1 text-xs font-medium" :class="billingBadgeClass(model.pricing?.billing_mode)">{{ billingLabel(model.pricing?.billing_mode) }}</span>
              <span v-for="group in model.groups.slice(0, 3)" :key="group.id" class="rounded-md bg-gray-100 px-2 py-1 text-xs text-gray-600 dark:bg-dark-700 dark:text-gray-300">{{ group.name }}</span>
              <span v-if="model.groups.length > 3" class="rounded-md bg-gray-100 px-2 py-1 text-xs text-gray-500 dark:bg-dark-700">+{{ model.groups.length - 3 }}</span>
            </div>
            <p class="mt-3 truncate text-xs text-gray-400">{{ model.channels.join(' / ') }}</p>
          </article>
        </div>

        <div v-else class="overflow-x-auto border border-gray-200 dark:border-dark-700">
          <table class="w-full min-w-[760px] text-sm">
            <thead class="bg-gray-50 text-left text-xs font-medium uppercase text-gray-500 dark:bg-dark-800">
              <tr><th class="px-4 py-3">{{ t('modelMarketplace.model') }}</th><th class="px-4 py-3">{{ t('modelMarketplace.platform') }}</th><th class="px-4 py-3">{{ t('modelMarketplace.groups') }}</th><th class="px-4 py-3">{{ t('modelMarketplace.pricing') }}</th><th class="w-12 px-4 py-3"></th></tr>
            </thead>
            <tbody class="divide-y divide-gray-100 dark:divide-dark-700">
              <tr v-for="model in filteredModels" :key="model.key" class="hover:bg-gray-50 dark:hover:bg-dark-800/70">
                <td class="px-4 py-3 font-medium text-gray-900 dark:text-white">{{ model.name }}</td>
                <td class="px-4 py-3 text-gray-600 dark:text-gray-300">{{ model.platform }}</td>
                <td class="px-4 py-3 text-gray-500">{{ model.groups.map((group) => group.name).join('、') }}</td>
                <td class="px-4 py-3 text-gray-500">{{ priceSummary(model) }}</td>
                <td class="px-4 py-3"><button class="btn-ghost btn-icon" :title="t('modelMarketplace.copyModel')" @click="copyModel(model.name)"><Icon name="copy" size="sm" /></button></td>
              </tr>
            </tbody>
          </table>
        </div>
      </main>
    </div>
  </AppLayout>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import AppLayout from '@/components/layout/AppLayout.vue'
import Icon from '@/components/icons/Icon.vue'
import PlatformIcon from '@/components/common/PlatformIcon.vue'
import userChannelsAPI, { type UserAvailableGroup, type UserSupportedModelPricing } from '@/api/channels'
import { useAppStore } from '@/stores/app'
import { extractApiErrorMessage } from '@/utils/apiError'
import type { BillingMode } from '@/constants/channel'
import type { GroupPlatform } from '@/types'

interface MarketModel {
  key: string
  name: string
  platform: string
  pricing: UserSupportedModelPricing | null
  groups: UserAvailableGroup[]
  channels: string[]
}

const { t } = useI18n()
const appStore = useAppStore()
const models = ref<MarketModel[]>([])
const loading = ref(false)
const query = ref('')
const selectedProvider = ref('all')
const selectedGroup = ref(0)
const viewMode = ref<'grid' | 'table'>('grid')
const copiedModel = ref('')

const providers = computed(() => {
  const counts = new Map<string, number>()
  for (const model of models.value) counts.set(model.platform, (counts.get(model.platform) || 0) + 1)
  return [
    { value: 'all', label: t('modelMarketplace.allProviders'), count: models.value.length },
    ...Array.from(counts.entries()).sort(([a], [b]) => a.localeCompare(b)).map(([value, count]) => ({ value, label: value, count })),
  ]
})

const groups = computed(() => {
  const counts = new Map<number, { id: number; name: string; count: number }>()
  for (const model of models.value) {
    for (const group of model.groups) {
      const current = counts.get(group.id)
      if (current) current.count += 1
      else counts.set(group.id, { id: group.id, name: group.name, count: 1 })
    }
  }
  return [{ id: 0, name: t('modelMarketplace.allGroups'), count: models.value.length }, ...Array.from(counts.values()).sort((a, b) => a.name.localeCompare(b.name))]
})

const filteredModels = computed(() => {
  const needle = query.value.trim().toLowerCase()
  return models.value.filter((model) => {
    if (selectedProvider.value !== 'all' && model.platform !== selectedProvider.value) return false
    if (selectedGroup.value && !model.groups.some((group) => group.id === selectedGroup.value)) return false
    if (!needle) return true
    return model.name.toLowerCase().includes(needle) || model.platform.toLowerCase().includes(needle) || model.groups.some((group) => group.name.toLowerCase().includes(needle)) || model.channels.some((channel) => channel.toLowerCase().includes(needle))
  })
})

async function loadData() {
  loading.value = true
  try {
    const channels = await userChannelsAPI.getAvailable()
    const byKey = new Map<string, MarketModel>()
    for (const channel of channels) {
      for (const section of channel.platforms) {
        for (const supported of section.supported_models) {
          const key = `${section.platform}:${supported.name}`
          const existing = byKey.get(key)
          if (existing) {
            for (const group of section.groups) if (!existing.groups.some((item) => item.id === group.id)) existing.groups.push(group)
            if (!existing.channels.includes(channel.name)) existing.channels.push(channel.name)
            if (!existing.pricing && supported.pricing) existing.pricing = supported.pricing
          } else {
            byKey.set(key, { key, name: supported.name, platform: section.platform, pricing: supported.pricing, groups: [...section.groups], channels: [channel.name] })
          }
        }
      }
    }
    models.value = Array.from(byKey.values()).sort((a, b) => a.name.localeCompare(b.name))
  } catch (error: unknown) {
    appStore.showError(extractApiErrorMessage(error, t('common.error')))
  } finally {
    loading.value = false
  }
}

function resetFilters() { query.value = ''; selectedProvider.value = 'all'; selectedGroup.value = 0 }
async function copyModel(name: string) { await navigator.clipboard.writeText(name); copiedModel.value = name; window.setTimeout(() => { if (copiedModel.value === name) copiedModel.value = '' }, 1500) }
function money(value: number | null | undefined) { return value == null ? null : `$${value.toFixed(4)}` }
function priceSummary(model: MarketModel) {
  const pricing = model.pricing
  if (!pricing) return t('modelMarketplace.noPricing')
  if (pricing.billing_mode === 'per_request') return `${t('modelMarketplace.perRequest')} ${money(pricing.per_request_price) || '-'}`
  if (pricing.billing_mode === 'image') return `${t('modelMarketplace.imageOutput')} ${money(pricing.image_output_price) || '-'}`
  const parts = [pricing.input_price != null ? `${t('modelMarketplace.input')} ${money(pricing.input_price)}` : '', pricing.output_price != null ? `${t('modelMarketplace.output')} ${money(pricing.output_price)}` : ''].filter(Boolean)
  return parts.length ? parts.join(' · ') : t('modelMarketplace.noPricing')
}
function billingLabel(mode?: BillingMode) { return mode ? t(`modelMarketplace.billing.${mode}`) : t('modelMarketplace.billing.unknown') }
function billingBadgeClass(mode?: BillingMode) { if (mode === 'image') return 'bg-blue-50 text-blue-700 dark:bg-blue-900/20 dark:text-blue-300'; if (mode === 'per_request') return 'bg-amber-50 text-amber-700 dark:bg-amber-900/20 dark:text-amber-300'; return 'bg-purple-50 text-purple-700 dark:bg-purple-900/20 dark:text-purple-300' }

onMounted(loadData)
</script>
