import {Injectable} from '@angular/core';

@Injectable({
  providedIn: 'root'
})
export class BalancerService {

  public getIP(): string {
    return window.location.origin;
  }
}
