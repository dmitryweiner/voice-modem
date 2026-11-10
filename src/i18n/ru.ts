import type { Messages } from './en';
import { plural, signed } from './plural';

export const ru: Messages = {
  title: 'Голосовой модем',
  description: 'Акустический модем в браузере: текст и небольшие файлы по любому голосовому каналу.',
  intro: 'Текст звуком. Откройте эту страницу на двух устройствах рядом, включите на обоих микрофон и отправьте текст с одного из них.',
  languageLabel: 'Язык',
  source: 'Исходный код',
  coreLoading: 'Ядро модема загружается…',
  coreFailed: 'Ядро модема не загрузилось.',

  micOn: 'Включить микрофон',
  micOff: 'Выключить',
  micStarting: 'Запуск…',
  micDenied: 'Браузер не разрешил доступ к микрофону. Разрешите его для этой страницы и попробуйте снова.',
  micNoDevice: 'Микрофон не найден.',
  micUnsupported: 'Этот браузер не умеет работать со звуком так, как нужно модему.',
  micFailed: 'Звуковое устройство не запустилось.',
  listening: 'Приём',
  transmitting: 'Передача…',
  inputLevel: 'Уровень входа',
  clipping: 'Слишком громко: убавьте громкость или раздвиньте устройства.',
  spectrum: 'Спектр входного сигнала, от 0 до 4 кГц',
  messages: 'Сообщения',
  noMessages: 'Пока пусто. Здесь появятся отправленные и принятые тексты.',
  messageLabel: 'Текст для отправки',
  send: 'Отправить',
  received: 'Принято',
  sending: 'Отправляется…',
  sent: 'Отправлено',
  frameLost: 'Сообщение услышано, но прочитать его не удалось.',
  sendFailed: 'Текст не отправлен.',

  coreReady: (version: string) => `Ядро модема ${version} готово.`,
  tooLong: (bytes: number) =>
    `Длиннее допустимого на ${bytes} ${plural('ru', bytes, { one: 'байт', few: 'байта', many: 'байт', other: 'байта' })}.`,
  snr: (db: number) => `сигнал/шум ${signed(db)} дБ`,
  seconds: (seconds: number) => `${seconds.toFixed(1).replace('.', ',')} с`,
};
